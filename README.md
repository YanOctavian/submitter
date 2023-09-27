# submitter

## Introduction

Responsible for the profit statistics of Ethereum L2 cross-chain transactions, 
and submits the Merkel root of the profit data to the Ethereum L1 chain. 
Completely decentralized.
## Installation

### Docker
todo

### Local

1. install git
```shell
sudo apt install git
```
2. install rust
```angular2html
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```


3. install clang and llvm
```shell
sudo apt install clang llvm
```
4. install tmux
```shell
sudo apt install tmux
```
5. build submitter
```angular2html
git clone https://github.com/Orbiter-Finance/OB_Submitter.git 
cd submitter
cargo build --release
```
6. configure environment variables
```shell
# Ethereum node https url
export NETWORK_RPC_URL = "https://eth-mainnet.g.alchemy.com/v2/-e53KLekEWi1sTblk40_P9z_qr_jTHTd"
# Ethereum network chain id
export MAINNET_CHAIN_ID=1

# RFeeManager smart contract address
export ORFeeManager_CONTRACT_ADDRESS="0xcC45c67d121A4BC3BC88Cc5A3099a8F7854f11f6"
# where to get txs
export TXS_SOURCE_URL="https://openapi.orbiter.finance/explore/v3/yj6toqvwh1177e1sexfy0u1pxx5j8o47"
# Where to get chain information
export SUPPORT_CHAINS_SOURCE_URL="https://api.studio.thegraph.com/query/53475/dealer/version/latest"

# Use directly without changing
export START_BLOCK=18212105
# Use directly without changing
export ZK_DELAY_SECONDS=28800
# Use directly without changing
export OP_DELAY_SECONDS=604800
# Use directly without changing
export COMMON_DELAY_SECONDS=900
```
7. Prepare private key  
   In your blockchain wallet, obtain the EOA private key, such as `0xed0e10acdb4b9ad17a0d9ec1b6f92d9e70d9f9c0bbfc609eb1aa03a370aba499`

9. run submitter
```angular2html
./target/release/submitter
```
copy and input your private key.
> If you don't want to be a submitter and just want to sync data, then you can use `--no-private-key` in your command line.
> for example, `./target/release/submitter --no-private-key`
10. view log
```shell
# for example
tail -f -n 100 db/logs/submitter.log.2023-09-20
```




