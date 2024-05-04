pragma solidity ^0.8.7;

import {ResultsConsumer} from "./ResultsConsumer.sol";
import {NativeTokenSender} from "./ccip/NativeTokenSender.sol";
import {AutomationCompatibleInterface} from "@chainlink/contracts/src/v0.8/AutomationCompatible.sol";

struct Config {
  address oracle;
  address ccipRouter;
  address link;
  address weth9Token;
  address exchangeToken;
  address uniswapV3Router;
  uint64 subscriptionId;
  uint64 destinationChainSelector;
  uint32 gasLimit;
  bytes secrets;
  string source;
}

contract SportsPredictionGame is ResultsConsumer, NativeTokenSender, AutomationCompatibleInterface {
  uint256 private constant MIN_WAGER = 0.00001 ether;
  uint256 private constant MAX_WAGER = 0.01 ether;
  uint256 private constant GAME_RESOLVE_DELAY = 2 hours;

  mapping(uint256 => Game) private games;
  mapping(address => mapping(uint256 => Prediction[])) private predictions;

  mapping(uint256 => bytes32) private pendingRequests;

  uint256[] private activeGames;
  uint256[] private resolvedGames;

  struct Game {
    uint256 sportId;
    uint256 externalId;
    uint256 timestamp;
    uint256 homeWagerAmount;
    uint256 awayWagerAmount;
    bool resolved;
    Result result;
  }

  struct Prediction {
    uint256 gameId;
    Result result;
    uint256 amount;
    bool claimed;
  }

  enum Result {
    None,
    Home,
    Away
  }

  event GameRegistered(uint256 indexed gameId);
  event GameResolved(uint256 indexed gameId, Result result);
  event Predicted(address indexed user, uint256 indexed gameId, Result result, uint256 amount);
  event Claimed(address indexed user, uint256 indexed gameId, uint256 amount);

  error GameAlreadyRegistered();
  error TimestampInPast();
  error GameNotRegistered();
  error GameIsResolved();
  error GameAlreadyStarted();
  error InsufficientValue();
  error ValueTooHigh();
  error InvalidResult();
  error GameNotResolved();
  error GameNotReadyToResolve();
  error ResolveAlreadyRequested();
  error NothingToClaim();

  constructor(
    Config memory config
  )
    ResultsConsumer(config.oracle, config.subscriptionId, config.source, config.secrets, config.gasLimit)
    NativeTokenSender(
      config.ccipRouter,
      config.link,
      config.weth9Token,
      config.exchangeToken,
      config.uniswapV3Router,
      config.destinationChainSelector
    )
  {}

  function predict(uint256 gameId, Result result) public payable {
    Game memory game = games[gameId];
    uint256 wagerAmount = msg.value;

    if (game.externalId == 0) revert GameNotRegistered();
    if (game.resolved) revert GameIsResolved();
    if (game.timestamp < block.timestamp) revert GameAlreadyStarted();
    if (wagerAmount < MIN_WAGER) revert InsufficientValue();
    if (wagerAmount > MAX_WAGER) revert ValueTooHigh();

    if (result == Result.Home) games[gameId].homeWagerAmount += wagerAmount;
    else if (result == Result.Away) games[gameId].awayWagerAmount += wagerAmount;
    else revert InvalidResult();

    predictions[msg.sender][gameId].push(Prediction(gameId, result, wagerAmount, false));
    emit Predicted(msg.sender, gameId, result, wagerAmount);
  }

  function registerAndPredict(uint256 sportId, uint256 externalId, uint256 timestamp, Result result) external payable {
    uint256 gameId = _registerGame(sportId, externalId, timestamp);
    predict(gameId, result);
  }

  function claim(uint256 gameId, bool transfer) external {
    Game memory game = games[gameId];
    address user = msg.sender;

    if (!game.resolved) revert GameNotResolved();

    uint256 totalWinnings = 0;
    Prediction[] memory userPredictions = predictions[user][gameId];
    for (uint256 i = 0; i < userPredictions.length; i++) {
      Prediction memory prediction = userPredictions[i];
      if (prediction.claimed) continue;
      if (game.result == Result.None) {
        totalWinnings += prediction.amount;
      } else if (prediction.result == game.result) {
        uint256 winnings = calculateWinnings(gameId, prediction.amount, prediction.result);
        totalWinnings += winnings;
      }
      predictions[user][gameId][i].claimed = true;
    }

    if (totalWinnings == 0) revert NothingToClaim();

    if (transfer) {
      _sendTransferRequest(user, totalWinnings);
    } else {
      payable(user).transfer(totalWinnings);
    }

    emit Claimed(user, gameId, totalWinnings);
  }

  function _registerGame(uint256 sportId, uint256 externalId, uint256 timestamp) internal returns (uint256 gameId) {
    gameId = getGameId(sportId, externalId);

    if (games[gameId].externalId != 0) revert GameAlreadyRegistered();
    if (timestamp < block.timestamp) revert TimestampInPast();

    games[gameId] = Game(sportId, externalId, timestamp, 0, 0, false, Result.None);
    activeGames.push(gameId);

    emit GameRegistered(gameId);
  }

  function _requestResolve(uint256 gameId) internal {
    Game memory game = games[gameId];

    if (pendingRequests[gameId] != 0) revert ResolveAlreadyRequested();
    if (game.externalId == 0) revert GameNotRegistered();
    if (game.resolved) revert GameIsResolved();
    if (!readyToResolve(gameId)) revert GameNotReadyToResolve();

    pendingRequests[gameId] = _requestResult(game.sportId, game.externalId);
  }

  function _processResult(uint256 sportId, uint256 externalId, bytes memory response) internal override {
    uint256 gameId = getGameId(sportId, externalId);
    Result result = Result(uint256(bytes32(response)));
    _resolveGame(gameId, result);
  }

  function _resolveGame(uint256 gameId, Result result) internal {
    games[gameId].result = result;
    games[gameId].resolved = true;

    resolvedGames.push(gameId);
    _removeFromActiveGames(gameId);

    emit GameResolved(gameId, result);
  }

  function _removeFromActiveGames(uint256 gameId) internal {
    uint256 index;
    for (uint256 i = 0; i < activeGames.length; i++) {
      if (activeGames[i] == gameId) {
        index = i;
        break;
      }
    }
    for (uint256 i = index; i < activeGames.length - 1; i++) {
      activeGames[i] = activeGames[i + 1];
    }
    activeGames.pop();
  }

  function getGameId(uint256 sportId, uint256 externalId) public pure returns (uint256) {
    return (sportId << 128) | externalId;
  }

  function getGame(uint256 gameId) external view returns (Game memory) {
    return games[gameId];
  }

  function getActiveGames() public view returns (Game[] memory) {
    Game[] memory activeGamesArray = new Game[](activeGames.length);
    for (uint256 i = 0; i < activeGames.length; i++) {
      activeGamesArray[i] = games[activeGames[i]];
    }
    return activeGamesArray;
  }

  function getActivePredictions(address user) external view returns (Prediction[] memory) {
    uint256 totalPredictions = 0;
    for (uint256 i = 0; i < activeGames.length; i++) {
      totalPredictions += predictions[user][activeGames[i]].length;
    }
    uint256 index = 0;
    Prediction[] memory userPredictions = new Prediction[](totalPredictions);
    for (uint256 i = 0; i < activeGames.length; i++) {
      Prediction[] memory gamePredictions = predictions[user][activeGames[i]];
      for (uint256 j = 0; j < gamePredictions.length; j++) {
        userPredictions[index] = gamePredictions[j];
        index++;
      }
    }
    return userPredictions;
  }

  function getPastPredictions(address user) external view returns (Prediction[] memory) {
    uint256 totalPredictions = 0;
    for (uint256 i = 0; i < resolvedGames.length; i++) {
      totalPredictions += predictions[user][resolvedGames[i]].length;
    }
    uint256 index = 0;
    Prediction[] memory userPredictions = new Prediction[](totalPredictions);
    for (uint256 i = 0; i < resolvedGames.length; i++) {
      Prediction[] memory gamePredictions = predictions[user][resolvedGames[i]];
      for (uint256 j = 0; j < gamePredictions.length; j++) {
        userPredictions[index] = gamePredictions[j];
        index++;
      }
    }
    return userPredictions;
  }

  function isPredictionCorrect(address user, uint256 gameId, uint32 predictionIdx) external view returns (bool) {
    Game memory game = games[gameId];
    if (!game.resolved) return false;
    Prediction memory prediction = predictions[user][gameId][predictionIdx];
    return prediction.result == game.result;
  }

  function calculateWinnings(uint256 gameId, uint256 wager, Result result) public view returns (uint256) {
    Game memory game = games[gameId];
    uint256 totalWager = game.homeWagerAmount + game.awayWagerAmount;
    uint256 winnings = (wager * totalWager) / (result == Result.Home ? game.homeWagerAmount : game.awayWagerAmount);
    return winnings;
  }

  function readyToResolve(uint256 gameId) public view returns (bool) {
    return games[gameId].timestamp + GAME_RESOLVE_DELAY < block.timestamp;
  }

  function checkUpkeep(bytes memory) public view override returns (bool, bytes memory) {
    Game[] memory activeGamesArray = getActiveGames();
    for (uint256 i = 0; i < activeGamesArray.length; i++) {
      uint256 gameId = getGameId(activeGamesArray[i].sportId, activeGamesArray[i].externalId);
      if (readyToResolve(gameId) && pendingRequests[gameId] == 0) {
        return (true, abi.encodePacked(gameId));
      }
    }
    return (false, "");
  }

  function performUpkeep(bytes calldata data) external override {
    uint256 gameId = abi.decode(data, (uint256));
    _requestResolve(gameId);
  }

  function deletePendingRequest(uint256 gameId) external onlyOwner {
    delete pendingRequests[gameId];
  }
}
