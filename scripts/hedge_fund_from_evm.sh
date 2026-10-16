#!/usr/bin/env bash
# Move the treasury's HyperEVM-testnet USDC (Circle faucet) into HyperCore
# testnet perps collateral, so the hedge can place its first real order.
#
# Path verified 2026-10-13 against the chain itself: HyperCore testnet token
# USDC (index 0) is linked to evmContract 0x0B80…C206, Circle's
# CoreDepositWallet, on HyperEVM testnet (chain 998). Depositing through it
# with destinationDex = 0 credits the perps account. A plain ERC-20 transfer
# to that address would NOT credit anything -- only deposit() does.
# Source: developers.circle.com/cctp/howtos/transfer-usdc-from-hyperevm-to-hypercore
#
#   scripts/hedge_fund_from_evm.sh gen              # make the margin wallet key (once)
#   scripts/hedge_fund_from_evm.sh check            # balances only, no key
#   scripts/hedge_fund_from_evm.sh fund <USDC>      # treasury -> margin wallet on HyperEVM, inside the share rule
#   scripts/hedge_fund_from_evm.sh deposit [USDC]    # margin wallet -> HyperCore perps (default: all)
#
# docs/46 #7 (2026-10-13): the principal key never deposits any more. A
# separate margin wallet (~/.tameion/hedge-margin.key, mode 600) is the only
# key that talks to the venue's deposit contract, and it can only ever hold
# what `fund` moved to it; `fund` signs with the treasury key once per top-up
# and refuses to exceed MAX_MARGIN_SHARE of all the USDC it can see. The
# margin wallet is therefore the venue account: HEDGE_USER for the hedge, and
# the master that approves the API wallet (TAMEION_TREASURY_KEY_FILE=
# ~/.tameion/hedge-margin.key for `approve_agent`). Its maximum loss is its
# balance, nothing more.
#
# Needs test HYPE for gas on HyperEVM testnet in whichever wallet signs.
#
# LEARNED 2026-10-13, the hard way: this only credits an address that already
# EXISTS on HyperCore -- and on testnet, existence mirrors MAINNET. Circle's
# CCTP-on-HyperCore docs (testnet section): "The recipient address must already
# exist on HyperCore mainnet ... Transfers to addresses without mainnet state
# fail silently." Our 20 USDC went into Circle's wallet and the Core-side
# sendAsset to the treasury was dropped (`userRole` = missing). Run `check`
# and confirm `userRole` is not "missing" before `deposit`; otherwise deposit
# at least 5 USDC on MAINNET with this address first.
set -euo pipefail
cd "$(dirname "$0")/.."

EVM_RPC=${EVM_RPC:-https://rpc.hyperliquid-testnet.xyz/evm}
INFO=https://api.hyperliquid-testnet.xyz/info
USDC=0x2B3370eE501B4a559b57D449569354196457D8Ab          # native USDC, HyperEVM testnet
DEPOSIT_WALLET=0x0B80659a4076E9E93C7DbE0f10675A16a3e5C206 # Circle CoreDepositWallet, testnet
PERPS=0                                                   # destinationDex: 0 = perps, 4294967295 = spot
# docs/46 #7: the venue may hold at most this share of the treasury's USDC
# (HyperEVM balance + what is already on the venue). Same default as
# `HedgePolicy::max_margin_share_pct`; a test pins the two together.
MAX_MARGIN_SHARE=${MAX_MARGIN_SHARE:-50}
TREASURY=$(tr -d '[:space:]' < ~/.tameion/treasury.address)
TREASURY_KEY_FILE=~/.tameion/treasury.key
MARGIN_KEY_FILE=${MARGIN_KEY_FILE:-$HOME/.tameion/hedge-margin.key}
MARGIN_ADDR_FILE=${MARGIN_ADDR_FILE:-$HOME/.tameion/hedge-margin.address}
MARGIN=$( [ -f "$MARGIN_ADDR_FILE" ] && tr -d '[:space:]' < "$MARGIN_ADDR_FILE" || echo "" )

usdc6() { python3 -c "import sys;print(f'{int(sys.argv[1])/1e6:.6f}')" "$1"; }
core_value() {  # $1 = address
  curl -s --max-time 20 -X POST "$INFO" -H 'Content-Type: application/json' \
    -d "{\"type\":\"clearinghouseState\",\"user\":\"$1\"}" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["marginSummary"]["accountValue"])'
}
evm_usdc() { cast call "$USDC" 'balanceOf(address)(uint256)' "$1" --rpc-url "$EVM_RPC" | awk '{print $1}'; }
balances() {
  echo "chain id        $(cast chain-id --rpc-url "$EVM_RPC")"
  echo "treasury        $TREASURY"
  echo "  HyperEVM HYPE $(cast balance "$TREASURY" --rpc-url "$EVM_RPC" --ether)  (gas)"
  echo "  HyperEVM USDC $(usdc6 "$(evm_usdc "$TREASURY")")"
  if [ -n "$MARGIN" ]; then
    echo "margin wallet   $MARGIN"
    echo "  HyperEVM HYPE $(cast balance "$MARGIN" --rpc-url "$EVM_RPC" --ether)  (gas)"
    echo "  HyperEVM USDC $(usdc6 "$(evm_usdc "$MARGIN")")"
    echo "  allowance     $(usdc6 "$(cast call "$USDC" 'allowance(address,address)(uint256)' "$MARGIN" "$DEPOSIT_WALLET" --rpc-url "$EVM_RPC" | awk '{print $1}')")  (to CoreDepositWallet)"
    echo "  HyperCore perps accountValue $(core_value "$MARGIN") USDC"
  else
    echo "margin wallet   none yet: run '$0 gen'"
  fi
}
# The share rule: after the move, the margin side (margin wallet on HyperEVM +
# its venue account) may hold at most MAX_MARGIN_SHARE % of all USDC seen.
share_check() {  # $1 = amount about to move onto the margin side (minor units)
  local on_margin total after limit
  on_margin=$(( $(evm_usdc "$MARGIN") + $(python3 -c "print(int(round(float('$(core_value "$MARGIN")')*1e6)))") ))
  total=$(( on_margin + $(evm_usdc "$TREASURY") ))
  after=$(( on_margin + $1 ))
  limit=$(( total * MAX_MARGIN_SHARE / 100 ))
  if [ "$after" -gt "$limit" ]; then
    echo "refusing: margin side after the move $(usdc6 "$after") USDC would exceed ${MAX_MARGIN_SHARE}% of $(usdc6 "$total") USDC (limit $(usdc6 "$limit")). docs/46 #7: the venue-side maximum loss must not grow past the share a human set. Move at most $(usdc6 $(( limit > on_margin ? limit - on_margin : 0 ))) USDC, or raise MAX_MARGIN_SHARE on purpose."
    exit 4
  fi
  echo "margin bound    $(usdc6 "$after") / $(usdc6 "$total") USDC on the margin side after the move, within ${MAX_MARGIN_SHARE}%"
}

need_margin() { [ -n "$MARGIN" ] || { echo "no margin wallet yet: run '$0 gen' first"; exit 5; }; }

case "${1:-}" in
gen)
  if [ -f "$MARGIN_KEY_FILE" ]; then echo "refusing: $MARGIN_KEY_FILE already exists (address $MARGIN). Remove it on purpose to rotate."; exit 2; fi
  umask 077
  out=$(cast wallet new)
  addr=$(awk '/Address:/{print $2}' <<<"$out")
  awk '/Private key:/{print $3}' <<<"$out" > "$MARGIN_KEY_FILE"
  chmod 600 "$MARGIN_KEY_FILE"
  printf '%s\n' "$addr" > "$MARGIN_ADDR_FILE"
  unset out
  echo "margin wallet   $addr"
  echo "key             $MARGIN_KEY_FILE (mode 600; never printed)"
  echo "next            send it a little test HYPE for gas; then '$0 fund <USDC>'; approve the API wallet with"
  echo "                TAMEION_TREASURY_KEY_FILE=$MARGIN_KEY_FILE cargo run --locked -p hedge --example approve_agent"
  echo "                and run the hedge with HEDGE_USER=$addr. On testnet the venue credits only addresses with mainnet state."
  ;;
check)
  balances
  for who in "$TREASURY" ${MARGIN:+"$MARGIN"}; do
    printf 'HyperCore userRole %s   ' "$who"; curl -s --max-time 20 -X POST "$INFO" -H 'Content-Type: application/json' -d "{\"type\":\"userRole\",\"user\":\"$who\"}"; echo
  done ;;
fund)
  need_margin; balances
  [ -n "${2:-}" ] || { echo "usage: $0 fund <USDC>"; exit 64; }
  amount=$(python3 -c "print(int(round(float('$2')*1e6)))")
  raw=$(evm_usdc "$TREASURY")
  [ "$amount" -gt 0 ] || { echo "nothing to move"; exit 1; }
  [ "$amount" -le "$raw" ] || { echo "asked $(usdc6 "$amount") but the treasury holds $(usdc6 "$raw") on HyperEVM"; exit 1; }
  share_check "$amount"
  gas=$(cast balance "$TREASURY" --rpc-url "$EVM_RPC")
  [ "$gas" != "0" ] || { echo "no HYPE for gas in the treasury on HyperEVM testnet"; exit 2; }
  PK=$(tr -d '[:space:]' < "$TREASURY_KEY_FILE")
  echo "== treasury -> margin wallet: transfer $(usdc6 "$amount") USDC (the principal key's only venue-side action) =="
  cast send "$USDC" 'transfer(address,uint256)' "$MARGIN" "$amount" \
    --rpc-url "$EVM_RPC" --private-key "$PK" --json | python3 -c 'import sys,json;r=json.load(sys.stdin);print("  tx",r["transactionHash"],"status",r["status"])'
  unset PK
  echo; balances ;;
deposit)
  need_margin; balances
  raw=$(evm_usdc "$MARGIN")
  if [ -n "${2:-}" ]; then amount=$(python3 -c "print(int(round(float('$2')*1e6)))"); else amount=$raw; fi
  [ "$amount" -gt 0 ] || { echo "nothing to deposit: fund the margin wallet first ('$0 fund <USDC>')"; exit 1; }
  [ "$amount" -le "$raw" ] || { echo "asked $(usdc6 "$amount") but the margin wallet holds $(usdc6 "$raw") on HyperEVM"; exit 1; }
  role=$(curl -s --max-time 20 -X POST "$INFO" -H 'Content-Type: application/json' -d "{\"type\":\"userRole\",\"user\":\"$MARGIN\"}")
  case "$role" in *missing*) echo "refusing: $MARGIN has no HyperCore account (userRole=$role). Core drops deposits to addresses without mainnet state. Deposit >=5 USDC on MAINNET with this address first."; exit 3;; esac
  # Moving HyperEVM -> HyperCore inside the margin side changes nothing for
  # the share rule, but the deposit still proves it holds with amount 0.
  share_check 0
  gas=$(cast balance "$MARGIN" --rpc-url "$EVM_RPC")
  [ "$gas" != "0" ] || { echo "no HYPE for gas in the margin wallet on HyperEVM testnet; send it a little test HYPE first"; exit 2; }
  PK=$(tr -d '[:space:]' < "$MARGIN_KEY_FILE")
  echo
  echo "== approve $(usdc6 "$amount") USDC to CoreDepositWallet (margin wallet signs; the principal key is not involved) =="
  cast send "$USDC" 'approve(address,uint256)' "$DEPOSIT_WALLET" "$amount" \
    --rpc-url "$EVM_RPC" --private-key "$PK" --json | python3 -c 'import sys,json;r=json.load(sys.stdin);print("  tx",r["transactionHash"],"status",r["status"])'
  echo "== deposit(amount, destinationDex=$PERPS) =="
  cast send "$DEPOSIT_WALLET" 'deposit(uint256,uint32)' "$amount" "$PERPS" \
    --rpc-url "$EVM_RPC" --private-key "$PK" --json | python3 -c 'import sys,json;r=json.load(sys.stdin);print("  tx",r["transactionHash"],"status",r["status"])'
  unset PK
  echo "== wait for HyperCore to credit =="
  for i in $(seq 1 30); do v=$(core_value "$MARGIN"); echo "  accountValue $v"; [ "$v" != "0.0" ] && break; sleep 3; done
  echo; balances ;;
*)
  sed -n '2,20p' "$0"; exit 64 ;;
esac
