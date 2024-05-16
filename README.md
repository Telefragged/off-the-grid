<h1 align="center">Off the Grid</h1>

Decentralized grid trading on [Ergo](https://ergoplatform.org/).

<!--toc:start-->
- [Overview](#overview)
  - [What it does](#what-it-does)
- [Getting started](#getting-started)
  - [Building](#building)
  - [Node setup](#node-setup)
  - [Generate scans config](#generate-scans-config)
  - [Fetch token information (Optional)](#fetch-token-information-optional)
- [Using the applcation](#using-the-applcation)
  - [Creating grid orders](#creating-grid-orders)
  - [Redeeming grid orders](#redeeming-grid-orders)
  - [Viewing grid orders](#viewing-grid-orders)
  - [Help?](#help)
- [Running the matching bot](#running-the-matching-bot)
<!--toc:end-->

## Overview

### What it does

Off the Grid is a decentralized application built on the Ergo blockchain to implement automated grid trading orders while allowing users to retain controls of their funds.
It builds on of the grid trading contract described by kushti [here](https://www.ergoforum.org/t/decentralized-grid-trading-on-ergo/3750)
and implements an execution bot/batcher that can automate order matching without user interaction.

Read more about grid trading on [Investopedia](https://www.investopedia.com/terms/g/grid-trading.asp).

In more detail;

What Off the Grid does:
- Use a [contract](./contracts/grid_multi/contract.es) which only allows spending if orders are correctly filled or with the order owner's signature. The contract keeps track of multiple orders at once.
- Use offchain bots/batchers to keep track of grid orders and match them against other sources of liquidity.
  - Currently, only Spectrum AMMs are used to match against orders. Other sources, such as the SigUSD bank, can also be implemented.
- Trade ERG against any token, while accumulating profits as ERGs.

In doing the above, grid orders profit from repeated execution of the same orders while bot operators profit from arbitraging the difference in price of the liquidity source and grid orders.

What it could do in the future:
- Trade in token/token pairs instead of ERG/token (There are multiple ways of handling transaction fees with different pros and cons).
- Accumulate profits in tokens over ERGs.
- Support different types of orders, such as limit orders.

What it probably won't do:
- Ensure safety. The grid order contract has not been audited and should not be entrusted with large amounts of assets.
- Provide lambos. Grid trading is not a guaranteed way to make a profit.

Please make sure the last points are especially well understood. You are the only one responsible for the safety of your assets.

## Getting started

### Building

The recommended way to build Off the Grid is by using [Nix](https://nixos.org/). After installation, simply run `nix build`.
The executable can then be found in `./result/bin/off-the-grid`

Alternatively it can be built using cargo, which is installed via [rustup](https://rustup.rs/). After installation run `cargo build`. The executable is found under `./target/debug/off-the-grid`. To build in release mode, pass `--release` to the command, which places the executable in `./target/release/off-the-grid`

### Node setup
Off the Grid communicates with an Ergo node using its http API. Configuration for this can be found in the [node config](./node_config.json). Make sure you change the `api_key` option.
It is recommended to [set up a personal node](https://docs.ergoplatform.com/node/install/).

The node must also have a configured Wallet. This is required even for the matcher as node scans don't seem to work otherwise.
To set up a wallet follow [this guide](https://docs.ergoplatform.com/node/wallet/).

For simpler setup consider trying [Satergo](https://satergo.com/).

### Generate scans config

When the node is set up and a wallet has been initialized scans can be generated with the following command
```shell
$ off-the-grid scans create-config
```

This will create `scan_config.json` in the current directory containing the existing or generated scans' ids.

If the wallet scan is finished or currently in progress the scans may not contain all existing boxes. To include them provide the `--rescan` option to trigger a rescan.

### Fetch token information (Optional)

It is also recommended to fetch token information. This is optional but enable the grid commands to show and accept token names and decimals:
```shell
$ off-the-grid tokens update
```

Note that this currently uses the explorer API (by default https://explorer.ergoplatform.com/) instead of the node's own blockchain API.
This is to avoid having to configure the extra indexer on the node. The tokens are fetched from the current set of Spectrum pools. As more tokens become available on Spectrum, rerun the command to keep the list up to date.

## Using the applcation

### Creating grid orders

`off-the-grid grid create` is used to create new grid orders.

If the order creation transaction is successfully generated a summary will be printed:
```shell
$ off-the-grid grid create -t COMET -v 10 -o 50 -r 50000-100000 -i comet
Spectrum N2T  4512.296035009 ERG  299859883 COMET    Spectrum N2T  4515.123264839 ERG  299672683 COMET
Wallet            8.29276498 ERG      1.71 SigUSD    MultiGrid              6.801 ERG     187200 COMET
Wallet           0.100025424 ERG                     Wallet           7.056325554 ERG      3.42 SigUSD
Wallet            8.29276498 ERG      1.71 SigUSD    Miner fee              0.001 ERG
```
After reviewing the transaction it can be confirmed or cancelled by following the on-screen prompt.

### Redeeming grid orders

Redeem orders using `off-the-grid grid redeem`:
```shell
$ off-the-grid grid redeem -i comet
MultiGrid  6.801 ERG  187200 COMET    Wallet       6.8 ERG  187200 COMET
                                      Miner fee  0.001 ERG
```

### Viewing grid orders

Listing existing orders is done using `off-the-grid grid list`:
```shell
$ off-the-grid grid list
comet | 16 Sell 34 Buy, Bid 67000 ERG/COMET Ask 65000 ERG/COMET, Profit 0 ERG (0 COMET), Total 6.801 ERG 187200 COMET
```

Details for a specific grid order are shown using `off-the-grid grid details`:
```shell
$ cargo run -- grid details -i comet
Sell 10200 COMET @ 50000 ERG/COMET
Sell 10400 COMET @ 51000 ERG/COMET
Sell 10600 COMET @ 52000 ERG/COMET
...
```

### Help?

For more information use `off-the-grid <command> --help` or `off-the-grid help <command>`

## Running the matching bot

To run the matcher a reward address must be configured.
This can be done via the environment variable `MATCHER_REWARD_ADDRESS` or via a matcher config:
```json
{
    "reward_address": "9..."
}
```
The reward address does not require being tied to the node's wallet.

The matcher can then be started:
```shell
$ off-the-grid matcher
Using reward address: 9...
```
For more configuration see the [matcher_config](./matcher_config.json).

The matcher will only print transaction IDs when order matching transactions are submitted, or errors when they happen.

Even when a transaction is submitted there is a possibility that it is never confirmed. There are many reasons this can happen but the most important thing to know is that multiple matchers will be competing for the same transactions. On Ergo, an input can only be spent by one transaction. In Off the Grid's case the grid orders are inputs and matching orders against liquidity sources are transactions.
