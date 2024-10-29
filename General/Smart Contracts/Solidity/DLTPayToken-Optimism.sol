// SPDX-License-Identifier: MIT

pragma solidity ^0.8.19;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import "@openzeppelin/contracts/security/Pausable.sol";
import "@openzeppelin/contracts/access/AccessControl.sol";
import "@eth-optimism/contracts-bedrock/contracts/universal/IOptimismMintableERC20.sol";

contract DLTPayToken is ERC20, ERC20Burnable, Pausable, AccessControl {
    bytes32 public constant PAUSER_ROLE = keccak256("PAUSER_ROLE");
    bytes32 public constant BRIDGE_ROLE = keccak256("BRIDGE_ROLE");

    // ---------------------------------------------------------------- //
    // required by optimism bridge
    /**
     * @notice Address of the corresponding version of this token on the remote chain.
     */
    address public immutable REMOTE_TOKEN;

    /**
     * @notice Address of the StandardBridge on this network.
     */
    address public immutable BRIDGE;

    /**
     * @notice Emitted whenever tokens are minted for an account.
     *
     * @param account Address of the account tokens are being minted for.
     * @param amount  Amount of tokens minted.
     */
    event Mint(address indexed account, uint256 amount);

    /**
     * @notice Emitted whenever tokens are burned from an account.
     *
     * @param account Address of the account tokens are being burned from.
     * @param amount  Amount of tokens burned.
     */
    event Burn(address indexed account, uint256 amount);

    // ---------------------------------------------------------------- //
    // Support for anyswap/multichain.org
    // according to https://docs.multichain.org/developer-guide/how-to-develop-under-anyswap-erc20-standards
    // and https://github.com/anyswap/chaindata/blob/main/AnyswapV6ERC20.sol
    address public immutable underlying;

    event LogSwapin(
        bytes32 indexed txhash,
        address indexed account,
        uint amount
    );
    event LogSwapout(
        address indexed account,
        address indexed bindaddr,
        uint amount
    );

    function mint(
        address to,
        uint256 amount
    ) external onlyRole(BRIDGE_ROLE) returns (bool) {
        _mint(to, amount);
        emit Mint(to, amount);
        return true;
    }

    function burn(
        address from,
        uint256 amount
    ) external onlyRole(BRIDGE_ROLE) returns (bool) {
        _burn(from, amount);
        emit Burn(from, amount);
        return true;
    }

    // For backwards compatibility
    function Swapin(
        bytes32 txhash,
        address account,
        uint256 amount
    ) external onlyRole(BRIDGE_ROLE) returns (bool) {
        _mint(account, amount);
        emit LogSwapin(txhash, account, amount);
        return true;
    }

    // For backwards compatibility
    function Swapout(uint256 amount, address bindaddr) external returns (bool) {
        require(bindaddr != address(0), "AnyswapV6ERC20: address(0)");
        _burn(msg.sender, amount);
        emit LogSwapout(msg.sender, bindaddr, amount);
        return true;
    }

    // ---------------------------------------------------------------- //
    // required by optimism bridge

    /**
     * @notice ERC165 interface check function.
     *
     * @param _interfaceId Interface ID to check.
     *
     * @return Whether or not the interface is supported by this contract.
     */
    function supportsInterface(
        bytes4 _interfaceId
    ) public view override(AccessControl) returns (bool) {
        // Interface corresponding to the legacy L2StandardERC20.
        bytes4 iface1 = type(ILegacyMintableERC20).interfaceId;
        // Interface corresponding to the updated OptimismMintableERC20 (this contract).
        bytes4 iface2 = type(IOptimismMintableERC20).interfaceId;
        return
            _interfaceId == iface1 ||
            _interfaceId == iface2 ||
            super.supportsInterface(_interfaceId);
    }

    /**
     * @custom:legacy
     * @notice Legacy getter for the remote token. Use REMOTE_TOKEN going forward.
     */
    function l1Token() public view returns (address) {
        return REMOTE_TOKEN;
    }

    /**
     * @custom:legacy
     * @notice Legacy getter for the bridge. Use BRIDGE going forward.
     */
    function l2Bridge() public view returns (address) {
        return BRIDGE;
    }

    /**
     * @custom:legacy
     * @notice Legacy getter for REMOTE_TOKEN.
     */
    function remoteToken() public view returns (address) {
        return REMOTE_TOKEN;
    }

    /**
     * @custom:legacy
     * @notice Legacy getter for BRIDGE.
     */
    function bridge() public view returns (address) {
        return BRIDGE;
    }

    // ---------------------------------------------------------------- //

    /**
     * @param _bridge      Address of the L2 standard bridge.
     * @param _remoteToken Address of the corresponding L1 token.
     */
    constructor(
        address _bridge,
        address _remoteToken
    ) ERC20("DLTPAY", "DLTPAY") {
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(PAUSER_ROLE, msg.sender);
        underlying = address(0);
        REMOTE_TOKEN = _remoteToken;
        BRIDGE = _bridge;
    }

    function pause() public onlyRole(PAUSER_ROLE) {
        _pause();
    }

    function unpause() public onlyRole(PAUSER_ROLE) {
        _unpause();
    }

    function _beforeTokenTransfer(
        address from,
        address to,
        uint256 amount
    ) internal override whenNotPaused {
        super._beforeTokenTransfer(from, to, amount);
    }
}
