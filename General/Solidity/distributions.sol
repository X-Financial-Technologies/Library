// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract Distributions {
    address public issuer;
    uint public totalBondValue;
    mapping(address => uint) public investorShares;
    uint public totalShares;

    event SharesRegistered(address investor, uint shares);
    event CouponDistributed(address investor, uint amountDistributed);

    modifier onlyIssuer() {
        require(msg.sender == issuer, "Only the issuer can perform this action.");
        _;
    }

    constructor(uint _totalBondValue) {
        issuer = msg.sender;
        totalBondValue = _totalBondValue;
    }

    function registerShares(address investor, uint shares) external onlyIssuer {
        require(totalShares + shares <= totalBondValue, "Cannot exceed total bond value.");
        investorShares[investor] += shares;
        totalShares += shares;
        emit SharesRegistered(investor, shares);
    }

    // Modified distributeCoupon function to allow any amount of ETH to be sent to any address.
    function distributeCoupon(address payable investor, uint amount) external payable onlyIssuer {
        require(address(this).balance >= amount, "Contract does not have enough balance.");
        require(amount > 0, "Amount must be greater than 0.");

        investor.transfer(amount);

        emit CouponDistributed(investor, amount);
    }

    // Function to allow the contract to receive ETH.
    receive() external payable {}
}
