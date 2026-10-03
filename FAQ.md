# Huahua Prediction FAQ

Huahua Prediction is an up/down price game for BTC and ETH on ChihuahuaChain, played at
[predict.chihuahua.wtf](https://predict.chihuahua.wtf). By using it you accept the
[Terms of Use](https://predict.chihuahua.wtf/#/terms).

## The game

**How does a round work?**
Each round lasts 5 minutes. You bet UP or DOWN on the *Next* round while it is open. When it
locks, the current price becomes the lock price; 5 minutes later the close price is recorded.
If the close is higher, UP wins; if lower, DOWN wins.

**How much do winners get?**
Winners share the whole pool of the round, minus the 2% fee, in proportion to their bets:
`payout = your bet × (total pool − fee) ÷ winning side`. The multiplier shown on each side is
an estimate that changes as people bet, until the round locks.

**When is my bet refunded?**
In full, with no fee, when:
- only one side has bets (nobody to play against);
- the close price equals the lock price;
- the round is not settled in time (for example if the keeper is down).

**Can I change or cancel a bet?** No. One bet per round per address, final once on-chain.

**What is the minimum bet?** 1 USDC.

**Do I have to claim my winnings?** Yes. Winnings and refunds stay in the contract until you
press *Collect* (or send `Claim { epochs }` to the contract). They do not expire.

## Money

**What do I bet with?**
Only USDC: USDC.inj, Circle's USDC issued on Injective, brought to ChihuahuaChain over IBC
from Osmosis. Its denom on ChihuahuaChain is
`ibc/2CAD00F0570FC6068204B0F4DB49B5752ABEC73B8604693C528B0D9A4417A4D1`. The contract rejects
any other token, including HUAHUA.

**What is HUAHUA used for?** Only gas. A bet costs about 12 HUAHUA in network fees.

**I have Noble USDC (or OSMO, ATOM…) on Osmosis. How do I play?**
Swap it to USDC.inj on Osmosis (Noble USDC swaps 1:1), then use the *Transfer* page of the
site to bring it to ChihuahuaChain. Withdraw the same way, back to Osmosis.

**Why not USDC from Noble?** Circle is winding down USDC on Noble: minting stops on
13 October 2026 and it is fully paused on 12 January 2027. USDC.inj is its successor in Cosmos.

**I sent USDC.inj straight from Injective and the game doesn't see it.**
IBC tokens are named after the path they took. USDC.inj sent directly from Injective arrives
as a different denom from USDC.inj that comes through Osmosis, and the game only takes the
Osmosis one. Send it back to Injective and route it through Osmosis.

**Where does the 2% fee go?** To the Chihuahua ecosystem fund
(`chihuahua14fketv99hlrlk80mkggw643spsj3yyf7qylvqn`), only from rounds actually played.

## Prices and trust

**Where do prices come from?**
ChihuahuaChain has no on-chain oracle (Pyth dropped CosmWasm in August 2026). A keeper bot
takes BTC and ETH prices from seven public exchanges (Binance, Coinbase, Kraken, Bitstamp,
OKX, Bybit, Gemini), drops any quote more than 0.5% away from the median, and submits the
median of the rest if at least three are left. Otherwise the round is skipped and refunded.
The contract rejects prices older than 60 seconds.

**So I have to trust the keeper?**
For the price, yes: the keeper key could submit a wrong price. It cannot touch the funds,
move bets, or change who wins once a price is recorded. Every price is on-chain and can be
checked against the exchanges at the same timestamp.

**Who controls the contracts?**
- *Admin* (the ecosystem fund): can pause, change round length, minimum bet and fee, and
  collect the accrued fee to the treasury. No message of the contract lets it take players'
  bets or winnings. It is also the CosmWasm admin, so it could migrate the contracts to new
  code; any migration is public on-chain.
- *Operator* (the keeper): submits prices and runs rounds.
- The bet denom is fixed at instantiation and cannot be changed.

**Is the code open?** Yes, it is in this repository. See below to check that the deployed
contract matches it.

## Contracts

| | Address |
|---|---|
| BTC market | `chihuahua1awemkjhemtguexmqpatmtw3qrwn5avzd48e453cykzh45gswzwpqzrwgv7` |
| ETH market | `chihuahua1uwl7y5u2jmc78xklg2pvpryjngqfgrx6tc97suuhqekcqwpcnfdqpng24z` |
| Code id | `689` |

**How do I verify the deployed code?**
```sh
./scripts/build_wasm.sh                          # cosmwasm/optimizer 0.17.0, reproducible
cat artifacts/checksums.txt                      # sha256 of huahua_prediction.wasm
chihuahuad q wasm code-info 689 --node https://rpc.chihuahua.wtf:443   # data_hash
```
The `data_hash` on chain must equal the sha256 in `checksums.txt`; for the launch build it is
`175d1ee1e322ac949b5e7322808e19d475b4990f0ce488a00207061cc467f932`.

## Safety

**Is it legal where I live?** Betting on price movements is gambling and is restricted or
prohibited in many countries. It is your responsibility to check. Do not use a VPN to get
around restrictions.

**What can go wrong?** Smart contract bugs, chain or IBC outages, a wrong price from the
keeper, problems with USDC.inj itself, or mistakes with addresses. Only bet what you can afford
to lose.

**Is this financial advice?** No.

**I think I found a bug.** Please report it privately to the ChihuahuaChain team before
sharing it publicly.
