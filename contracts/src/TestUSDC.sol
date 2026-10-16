// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @title  TestUSDC
/// @notice A stand-in for Arc's native USDC, used only on a local anvil fork.
///
///         On Arc itself USDC lives at 0x3600000000000000000000000000000000000000
///         and is the native gas token as well as the settlement asset. That
///         precompile's transfer logic is implemented inside the Arc client
///         rather than in EVM bytecode, so an anvil fork can read its state
///         (balanceOf, symbol, decimals all answer correctly) but cannot execute
///         its transfer. Calls revert.
///
///         This contract exposes the identical surface the agent uses, so the
///         agent runs the same code path against either one and only the token
///         address changes. Real Arc emits standard ERC-20 Transfer events for
///         USDC movements, verified live against the testnet, which is what the
///         obligation model reads.
///
///         Not for deployment anywhere real. There is no access control on
///         mint by design: this exists to fund test wallets on a throwaway fork.
contract TestUSDC {
    string public constant name = "USD Coin (fork stand-in)";
    string public constant symbol = "USDC";
    uint8 public constant decimals = 6;

    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    error InsufficientBalance(uint256 available, uint256 required);
    error InsufficientAllowance(uint256 available, uint256 required);

    function mint(address to, uint256 amount) external {
        totalSupply += amount;
        balanceOf[to] += amount;
        emit Transfer(address(0), to, amount);
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        _move(msg.sender, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        if (allowed < amount) revert InsufficientAllowance(allowed, amount);
        if (allowed != type(uint256).max) {
            allowance[from][msg.sender] = allowed - amount;
        }
        _move(from, to, amount);
        return true;
    }

    function _move(address from, address to, uint256 amount) private {
        uint256 bal = balanceOf[from];
        if (bal < amount) revert InsufficientBalance(bal, amount);
        unchecked {
            balanceOf[from] = bal - amount;
            balanceOf[to] += amount;
        }
        emit Transfer(from, to, amount);
    }
}
