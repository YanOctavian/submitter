#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(unused_assignments)]

pub mod funcs;
pub mod rocks_db;
pub mod sled_db;

use crate::funcs::{SupportChains, TxsCrawler};
use contract::SubmitterContract;
use ethers::types::{Address, U256};
use primitives::error::Error;

use funcs::{calculate_profit, convert_string_to_hash, get_one_block_txs_hash};
use hex;
use primitives::{
    chain_type::get_chain_type,
    constants::ETH_DELAY_BLOCKS,
    env::{
        get_chains_info_source_url, get_delay_seconds_by_chain_type, get_mainnet_chain_id,
        get_txs_source_url,
    },
    func::{block_number_convert_to_h256, chain_token_address_convert_to_h256, tx_compare},
    traits::{Contract as ContractTrait, StataTrait},
    types::{
        BlockInfo, BlocksStateData, Chain, ChainType, CrossTxData, CrossTxProfit, Debt, Event,
        FeeManagerDuration, ProfitStateData, WithdrawEvent,
    },
};
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, CONNECTION, CONTENT_TYPE, USER_AGENT},
    Client, RequestBuilder,
};
use rocks_db::*;
use rocksdb::{
    ops::{Flush, Iterate, Open, WriteOps},
    IteratorMode, Options, ReadOptions, WriteBatch, DB,
};
use serde_json::{json, Value};
use sled::{self, Db, Tree};
use sled_db::*;
use sparse_merkle_tree::H256;
use state::{Keccak256Hasher, State};
use std::{
    cmp::{max, min},
    str::FromStr,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::broadcast::{Receiver, Sender};
use tracing::{event, info, level_to_log, span, Level};
use utils::vec_unique;

pub struct Submitter {
    profit_state: Arc<RwLock<State<'static, Keccak256Hasher, ProfitStateData>>>,
    blocks_state: Arc<RwLock<State<'static, Keccak256Hasher, BlocksStateData>>>,
    sled_db: Arc<Db>,
    rocks_db: Arc<TxsRocksDB>,
    contract: Arc<SubmitterContract>,
    start_block: Arc<RwLock<u64>>,
    db_path: String,
}

impl Submitter {
    pub fn new(
        profit_state: Arc<RwLock<State<'static, Keccak256Hasher, ProfitStateData>>>,
        blocks_state: Arc<RwLock<State<'static, Keccak256Hasher, BlocksStateData>>>,
        contract: Arc<SubmitterContract>,
        start_block: Arc<RwLock<u64>>,
        sled_db: Arc<Db>,
        rocks_db: Arc<TxsRocksDB>,
        db_path: String,
    ) -> Self {
        event!(Level::INFO, "rocks db is ready.");
        Self {
            profit_state,
            blocks_state,
            sled_db,
            rocks_db: rocks_db,
            contract,
            start_block,
            db_path,
        }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let _ = update_start_block_num(
            self.start_block.clone(),
            self.contract.clone(),
            self.blocks_state.clone(),
        )
        .await
        .unwrap();

        tokio::spawn(crawl_block_info(
            self.contract.sender.subscribe(),
            self.sled_db.clone(),
            self.start_block.clone(),
            self.contract.clone(),
        ));
        tokio::spawn(crawl_txs_and_calculate_profit_for_per_block(
            self.sled_db.clone(),
            self.rocks_db.clone(),
            self.db_path.clone(),
            self.start_block.clone(),
            self.contract.clone(),
        ));
        tokio::spawn(submit_root(
            self.contract.sender.subscribe(),
            self.sled_db.clone(),
            self.rocks_db.clone(),
            self.profit_state.clone(),
            self.blocks_state.clone(),
            self.contract.clone(),
            self.start_block.clone(),
        ));
        event!(Level::INFO, "submitter is ready.");
        std::future::pending::<()>().await;
        Ok(())
    }
}

async fn crawl_block_info(
    mut newest_block_receiver: Receiver<BlockInfo>,
    sled_db: Arc<Db>,
    start_block: Arc<RwLock<u64>>,
    contract: Arc<SubmitterContract>,
) -> anyhow::Result<()> {
    // let span = span!(Level::INFO, "crawl_block_info");
    // let _enter = span.enter();
    let block_info_db = ContractBlockInfoDB::new(sled_db.clone())?;
    let mut now_block = 0u64;
    {
        now_block = start_block.read().unwrap().clone();
    }
    if now_block == 0 {
        unreachable!()
    } else {
        now_block = now_block.saturating_sub(1);
    }
    event!(Level::INFO, "block info crawler is ready.");
    let profit_statistic_db = ProfitStatisticsDB::new(sled_db.clone())?;
    let user_tokens_db = UserTokensDB::new(sled_db.clone())?;
    loop {
        if let Ok(newest_block) = newest_block_receiver.recv().await {
            while now_block <= newest_block.storage.block_number - ETH_DELAY_BLOCKS {
                if block_info_db.get_block_info(now_block)?.is_none() {
                    match contract.get_block_info(now_block).await {
                        Ok(Some(now_block_info)) => {
                            block_info_db.insert_block_info(
                                now_block_info.storage.block_number,
                                now_block_info.clone(),
                            )?;

                            event!(
                                Level::INFO,
                                "Block #{:} info is saved.",
                                now_block_info.storage.block_number,
                            );
                            for e in now_block_info.events {
                                match e {
                                    Event::Withdraw(w_e) => {
                                        user_tokens_db.insert_token(
                                            w_e.address,
                                            w_e.chain_id,
                                            w_e.token_address,
                                        )?;
                                        profit_statistic_db.update_total_withdraw(
                                            w_e.address,
                                            w_e.chain_id,
                                            w_e.token_address,
                                            w_e.balance,
                                        )?;
                                    }
                                    Event::Deposit(d_e) => {
                                        user_tokens_db.insert_token(
                                            d_e.address,
                                            d_e.chain_id,
                                            d_e.token_address,
                                        )?;
                                        profit_statistic_db.update_total_deposit(
                                            d_e.address,
                                            d_e.chain_id,
                                            d_e.token_address,
                                            d_e.balance,
                                        )?;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            event!(
                                Level::WARN,
                                "Block #{:?} - get block info err: {:?}",
                                now_block,
                                e,
                            );
                            continue;
                        }
                        _ => {
                            continue;
                        }
                    }
                }
                now_block += 1;
            }
        }
    }
}

async fn crawl_txs_and_calculate_profit_for_per_block(
    sled_db: Arc<Db>,
    txs_db: Arc<TxsRocksDB>,
    db_path: String,
    start_block: Arc<RwLock<u64>>,
    contract: Arc<SubmitterContract>,
) -> anyhow::Result<()> {
    // let span = span!(Level::INFO, "crawl_txs");
    // let _enter = span.enter();
    let block_info_db = ContractBlockInfoDB::new(sled_db.clone())?;
    let block_txs_count_db = BlockTxsCountDB::new(sled_db.clone())?;
    let mut now_block = 0;
    {
        now_block = start_block.read().unwrap().clone();
    }

    if now_block == 0 {
        unreachable!()
    }

    let maker_profit_db = MakerProfitDB::new(sled_db.clone())?;
    let support_chains: Vec<u64> = vec_unique(
        SupportChains::new(get_chains_info_source_url())
            .get_support_chains()
            .await?,
    );
    println!("support chains: {:?}", support_chains);
    if support_chains.is_empty() {
        panic!("support chains is empty.");
    }

    event!(Level::INFO, "txs crawler is ready.");
    loop {
        if let Some(now_block_info) = block_info_db.get_block_info(now_block)? {
            if let Ok(Some(n)) = block_txs_count_db.get_count(now_block) {
                now_block += 1;
                continue;
            }

            let now_block_timestamp = now_block_info.storage.block_timestamp;
            let last_block_number = now_block_info.storage.block_number - 1;

            if let Some(last_block_info) = block_info_db.get_block_info(last_block_number)? {
                let last_block_timestamp = last_block_info.storage.block_timestamp;

                let mut new_txs: Vec<(CrossTxData, CrossTxProfit)> = Vec::new();
                match TxsCrawler::new(get_txs_source_url())
                    .request_txs(
                        support_chains.clone(),
                        last_block_timestamp,
                        now_block_timestamp,
                    )
                    .await
                {
                    Ok(txs) => {
                        if !txs.is_empty() {
                            event!(
                                Level::INFO,
                                "successfully obtained {:} pieces of txs from chains {:?}",
                                txs.clone().len(),
                                support_chains.clone(),
                            );
                        }
                        let mut tx_count = 0;
                        while tx_count < txs.len() {
                            let tx = txs[tx_count].clone();
                            let tx: CrossTxData = tx.into();
                            if let None = support_chains.iter().position(|p| p == &tx.target_chain)
                            {
                                event!(
                                    Level::WARN,
                                    "target chain id {:} is not support, continue",
                                    tx.target_chain,
                                );
                                tx_count += 1;
                                continue;
                            }
                            if let None = support_chains.iter().position(|p| p == &tx.source_chain)
                            {
                                tx_count += 1;
                                event!(
                                    Level::WARN,
                                    "source chain id {:} is not support, continue",
                                    tx.source_chain,
                                );
                                continue;
                            }
                            let token = tx.source_token;
                            let dealer = tx.dealer_address;
                            let mainnet_chain_id = get_mainnet_chain_id();
                            let mut percent = 0u64;
                            if let Some(p) =
                                maker_profit_db.get_percent(dealer, now_block, token)?
                            {
                                percent = p;
                            } else {
                                if let Ok(p) = contract
                                    .get_dealer_profit_percent_by_block(
                                        dealer,
                                        last_block_number,
                                        mainnet_chain_id,
                                        token,
                                    )
                                    .await
                                {
                                    maker_profit_db.insert_percent(dealer, now_block, token, p)?;
                                    percent = p;
                                } else {
                                    continue;
                                }
                            };
                            let profit = calculate_profit(percent as u64, tx.clone());
                            event!(
                                Level::INFO,
                                "Block #{:} - dealer {:} - profit percent: {:?}",
                                now_block,
                                dealer,
                                percent,
                            );

                            new_txs.push((tx.clone(), profit.clone()));
                            event!(
                                Level::INFO,
                                "Block #{:} - tx {:} - profit: {:?}",
                                now_block_info.storage.block_number,
                                hex::encode(tx.target_id.as_slice()),
                                profit,
                            );
                            tx_count += 1;
                        }
                    }
                    Err(e) => {
                        event!(
                            Level::WARN,
                            "get txs err: {:?}. start: {:?}, end: {:?}. chains: {:?}",
                            e,
                            last_block_timestamp,
                            now_block_timestamp,
                            support_chains.clone(),
                        );
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        continue;
                    }
                }
                // }
                now_block += 1;
                txs_db.insert_txs(new_txs.clone())?;
                block_txs_count_db
                    .insert_count(now_block_info.storage.block_number, new_txs.len() as u64)?;
                event!(
                    Level::INFO,
                    "Block #{:} - txs are saved. count: {:?}",
                    now_block_info.storage.block_number,
                    new_txs.len(),
                );
            } else {
                panic!("last block info is none.");
            }
        } else {
            event!(
                Level::INFO,
                "Block #{:} - block info has not been crawled yet.",
                now_block
            );
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

async fn submit_root(
    mut newest_block_receiver: Receiver<BlockInfo>,
    sled_db: Arc<Db>,
    txs_db: Arc<TxsRocksDB>,
    profit_state: Arc<RwLock<State<'static, Keccak256Hasher, ProfitStateData>>>,
    blocks_state: Arc<RwLock<State<'static, Keccak256Hasher, BlocksStateData>>>,
    contract: Arc<SubmitterContract>,
    start_block: Arc<RwLock<u64>>,
) -> anyhow::Result<()> {
    // let span = span!(Level::INFO, "submit_root");
    // let _enter = span.enter();
    let block_info_db = ContractBlockInfoDB::new(sled_db.clone())?;
    let block_txs_count_db = BlockTxsCountDB::new(sled_db.clone())?;
    let user_tokens_db = UserTokensDB::new(sled_db.clone())?;
    let profit_statistic_db = ProfitStatisticsDB::new(sled_db.clone())?;

    let mut newest_block_info = BlockInfo::default();
    let mut now_block_num = 0;
    let mut submit_root_block_num = 0;
    {
        now_block_num = start_block.read().unwrap().clone();
    }
    event!(Level::INFO, "submit root thread is ready.");
    loop {
        {
            if let Ok(info) = newest_block_receiver.recv().await {
                if submit_root_block_num >= info.storage.block_number {
                    continue;
                }
                newest_block_info = info
            }
        }
        let is_chilled =
            newest_block_info.clone().storage.duration == FeeManagerDuration::default();
        if !is_chilled {
            continue;
        }
        let trusted_block_num = newest_block_info.storage.block_number - ETH_DELAY_BLOCKS;
        event!(
            Level::INFO,
            "Chill time. Highest Block #{:?}",
            trusted_block_num,
        );
        let end_block_num = trusted_block_num - 2;
        if end_block_num <= now_block_num {
            continue;
        }

        if !block_txs_count_db.is_txs_completed(now_block_num, end_block_num)? {
            continue;
        }

        while now_block_num < end_block_num {
            let now_block_info_op = block_info_db.get_block_info(now_block_num)?;
            if now_block_info_op.is_none() {
                continue;
            }
            let now_block_info = now_block_info_op.unwrap();
            let last_block_info_op =
                block_info_db.get_block_info(now_block_num.checked_sub(1).unwrap())?;
            if last_block_info_op.is_none() {
                continue;
            }

            event!(
                Level::INFO,
                "Block #{:?}. - Archiving the dealer profit. - End Block #{:?}",
                now_block_num,
                end_block_num,
            );
            let last_block_info = last_block_info_op.unwrap();
            let timestamp_range = (
                last_block_info.storage.block_timestamp,
                now_block_info.storage.block_timestamp,
            );

            for e in now_block_info.events {
                match e.clone() {
                    Event::Withdraw(w_e) => {
                        let mut profit_state = profit_state.write().unwrap();
                        let user = chain_token_address_convert_to_h256(
                            w_e.chain_id,
                            w_e.token_address,
                            w_e.address,
                        );
                        let mut user_profit = profit_state.try_get(user).unwrap();
                        if user_profit == ProfitStateData::default() {
                            user_profit.token = w_e.token_address;
                            user_profit.token_chain_id = w_e.chain_id;
                        }
                        user_profit.sub_balance(w_e.balance).unwrap();
                        profit_state.try_update_all(vec![(user.clone(), user_profit.clone())])?;
                        event!(
                            Level::INFO,
                            "Block #{:?}. - withdraw event {:?} - user: {:?} - balance: {:?}",
                            now_block_num,
                            e,
                            user,
                            w_e.balance,
                        );
                    }
                    Event::Deposit(d_e) => {
                        let mut profit_state = profit_state.write().unwrap();
                        let user = chain_token_address_convert_to_h256(
                            d_e.chain_id,
                            d_e.token_address,
                            d_e.address,
                        );
                        let mut user_profit = profit_state.try_get(user).unwrap();
                        if user_profit == ProfitStateData::default() {
                            user_profit.token = d_e.token_address;
                            user_profit.token_chain_id = d_e.chain_id;
                        }
                        user_profit.add_balance(d_e.balance).unwrap();
                        profit_state.try_update_all(vec![(user.clone(), user_profit.clone())])?;
                        event!(
                            Level::INFO,
                            "Block #{:?}. - deposit event {:?} - user: {:?} - balance: {:?}",
                            now_block_num,
                            e,
                            user,
                            d_e.balance,
                        );
                    }
                }
            }

            let txs = txs_db.get_txs_by_timestamp_range(timestamp_range.0, timestamp_range.1)?;
            let mut tx_hashes: Vec<H256> = vec![];
            for tx in txs {
                let profit = tx.1.profit;
                if profit == U256::from(0) {
                    event!(
                        Level::INFO,
                        "Block #{:?}. - tx {:?} - profit is zero.",
                        now_block_num,
                        hex::encode(&tx.0.target_id.as_slice()),
                    );
                    continue;
                }
                let maker = tx.1.maker_address;
                let dealer = tx.1.dealer_address;
                let chain_id = tx.1.chain_id;
                let token_id = tx.1.token;
                let maker_key = chain_token_address_convert_to_h256(chain_id, token_id, maker);
                let dealer_key = chain_token_address_convert_to_h256(chain_id, token_id, dealer);
                let mut maker_profit = ProfitStateData::default();
                let mut dealer_profit = ProfitStateData::default();
                {
                    let b_s_r = profit_state.read().unwrap();
                    maker_profit = b_s_r.try_get(maker_key)?;
                    dealer_profit = b_s_r.try_get(dealer_key)?;
                }
                if maker_profit == ProfitStateData::default() {
                    maker_profit.token = token_id;
                    maker_profit.token_chain_id = chain_id;
                }
                maker_profit.sub_balance(profit).unwrap();
                if dealer_profit == ProfitStateData::default() {
                    dealer_profit.token = token_id;
                    dealer_profit.token_chain_id = chain_id;
                }
                profit_statistic_db.update_total_withdraw(maker, chain_id, token_id, profit)?;
                dealer_profit.add_balance(profit).unwrap();
                {
                    let mut profit_state = profit_state.write().unwrap();
                    profit_state.try_update_all(vec![
                        (maker_key, maker_profit.clone()),
                        (dealer_key, dealer_profit.clone()),
                    ])?;
                    event!(
                        Level::INFO,
                        "Block #{:?}. - tx {:?} - maker: {:?} - dealer: {:?} - profit: {:?}",
                        now_block_num,
                        hex::encode(&tx.0.target_id.as_slice()),
                        maker,
                        dealer,
                        profit,
                    );
                }
                profit_statistic_db.update_total_profit(dealer, chain_id, token_id, profit)?;
                user_tokens_db.insert_token(maker, chain_id, token_id)?;
                user_tokens_db.insert_token(dealer, chain_id, token_id)?;

                tx_hashes.push(tx.0.target_id);
            }

            let txs_hash = get_one_block_txs_hash(tx_hashes.clone());
            event!(
                Level::INFO,
                "Block #{:?}. - txs hash: {:?}",
                now_block_num,
                hex::encode(&txs_hash.as_slice()),
            );

            if now_block_num == 0 {
                unreachable!()
            }
            let mut b_w = blocks_state.write().unwrap();
            let last_key = block_number_convert_to_h256(now_block_num - 1);
            let now_key = block_number_convert_to_h256(now_block_num);
            let profit_root = profit_state.read().unwrap().try_get_root()?;
            let mut new_block = BlocksStateData {
                txs: txs_hash.into(),
                block_num: now_block_num,
                profit_root: profit_root.into(),
                ..Default::default()
            };
            let old_block = b_w.try_get(last_key)?;
            new_block.into_chain(old_block);
            b_w.try_update_all(vec![(now_key, new_block.clone())])?;
            event!(
                Level::INFO,
                "Block #{:?}. - block state add new block: {:?}",
                now_block_num,
                serde_json::to_string(&new_block).unwrap(),
            );
            now_block_num += 1;
        }

        let profit_root = profit_state.read().unwrap().try_get_root()?;
        let block_txs_root = blocks_state.read().unwrap().try_get_root()?;

        if sparse_merkle_tree::H256::from(newest_block_info.storage.profit_root) == profit_root {
            event!(Level::INFO, "root is not changed, pending......");
            continue;
        }
        event!(
            Level::INFO,
            "Block #{:?} submit root. - Start Block #{:?}, End Block #{:?} - profit root: {:?} - txs root: {:?}",
            newest_block_info.storage.block_number,
            newest_block_info.storage.last_update_block,
            end_block_num,
            hex::encode(&profit_root.as_slice()),
            hex::encode(&block_txs_root.as_slice()),
        );
        match contract
            .submit_root(
                newest_block_info.storage.last_update_block,
                end_block_num,
                profit_root.into(),
                block_txs_root.into(),
            )
            .await
        {
            Ok(r) => {
                event!(
                    Level::INFO,
                    "Block #{:?}, submit root hash: {:?}",
                    newest_block_info.storage.block_number,
                    r
                );
                if let Some(s) = r.1 {
                    submit_root_block_num = s.as_u64();
                }
            }
            Err(e) => {
                event!(
                    Level::WARN,
                    "Block #{:?}, submit root err: {:?}",
                    newest_block_info.storage.block_number,
                    e
                );
                match e {
                    Error::SubmitRootFailed(err, b) => {
                        if let Some(s) = b {
                            submit_root_block_num = s.as_u64();
                        }
                    }
                    _ => {}
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(12)).await;
    }
}

async fn update_start_block_num(
    start_block: Arc<RwLock<u64>>,
    contract: Arc<SubmitterContract>,
    blocks_state: Arc<RwLock<State<'static, Keccak256Hasher, BlocksStateData>>>,
) -> anyhow::Result<()> {
    let mut block_num = start_block.read().unwrap().clone();

    let mut newest_block_num = 0u64;
    loop {
        let mut r = contract.sender.subscribe();
        let newest_block = r.recv().await?;
        newest_block_num = newest_block.storage.block_number;
        if newest_block_num != 0 {
            break;
        }
    }
    let trusted_block_num = newest_block_num - ETH_DELAY_BLOCKS;

    if block_num > trusted_block_num {
        panic!("start block number too large.");
    }
    let b_s = blocks_state.read().unwrap();
    if b_s.try_get_root().unwrap() == H256::default() {
        return Ok(());
    }

    {
        let key = block_number_convert_to_h256(block_num);
        let value = b_s.try_get(key).unwrap();
        if value == Default::default() {
            panic!("start block number too large.");
        }
        block_num += 1;
    }

    let mut is_start_block_exists: bool = false;
    while block_num <= trusted_block_num {
        let key = block_number_convert_to_h256(block_num);
        let value = b_s.try_get(key)?;
        if value == Default::default() {
            let mut s_w = start_block.write().unwrap();
            *s_w = block_num;
            is_start_block_exists = true;
            break;
        } else {
            block_num = block_num.checked_add(1).expect("overflow");
        }
    }
    if !is_start_block_exists {
        panic!("wait a moment.");
    }

    event!(
        Level::INFO,
        "Start block updated successfully.  Start Block #{:?}",
        block_num,
    );
    println!("start block : {:?}", start_block.read().unwrap().clone());
    Ok(())
}
