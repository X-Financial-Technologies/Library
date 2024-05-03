// SPDX-License-Identifier: MIT

pragma solidity ^0.8.19;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import "@openzeppelin/contracts/security/Pausable.sol";
import "@openzeppelin/contracts/access/AccessControl.sol";
import "@arbitrum/token-bridge-contracts/contracts/tokenbridge/arbitrum/IArbToken.sol";

contract DLTPayToken is
    ERC20,
    ERC20Burnable,
    Pausable,
    AccessControl,
    IArbToken
{
    bytes32 public constant PAUSER_ROLE = keccak256("PAUSER_ROLE");
    bytes32 public constant BRIDGE_ROLE = keccak256("BRIDGE_ROLE");

    // ---------------------------------------------------------------- //
    // Support arbitrum
    address public l2Gateway;
    address public override l1Address;

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
    ) public onlyRole(BRIDGE_ROLE) returns (bool) {
        _mint(to, amount);
        return true;
    }

    function burn(
        address from,
        uint256 amount
    ) external onlyRole(BRIDGE_ROLE) returns (bool) {
        _burn(from, amount);
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
    // Support arbitrum
    function bridgeMint(
        address account,
        uint256 amount
    ) external virtual override onlyRole(BRIDGE_ROLE) {
        _mint(account, amount);
    }

    function bridgeBurn(
        address account,
        uint256 amount
    ) external virtual override onlyRole(BRIDGE_ROLE) {
        _burn(account, amount);
    }

    // ---------------------------------------------------------------- //

    constructor() ERC20("DLTPAY", "DLTPAY") {
        _grantRole(DEFAULT_ADMIN_ROLE, msg.sender);
        _grantRole(PAUSER_ROLE, msg.sender);
        underlying = address(0);
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
