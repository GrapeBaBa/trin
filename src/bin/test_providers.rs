use std::{fmt, fs, ops::Range, sync::Arc};

use anyhow::{anyhow, Result};
use clap::Parser;
use rand::{
    distributions::{Distribution, Uniform},
    thread_rng,
};
use ssz::Decode;
use surf::{Client, Config};
use tracing::{debug, info, warn};
use url::Url;

use ethportal_api::{types::execution::accumulator::EpochAccumulator, utils::bytes::hex_encode};
use portal_bridge::{
    api::execution::ExecutionApi, bridge::history::EPOCH_SIZE, cli::Provider,
    types::mode::BridgeMode, PANDAOPS_CLIENT_ID, PANDAOPS_CLIENT_SECRET,
};
use trin_utils::log::init_tracing_logger;
use trin_validation::{
    accumulator::MasterAccumulator,
    constants::{
        BERLIN_BLOCK_NUMBER, BYZANTIUM_BLOCK_NUMBER, CONSTANTINOPLE_BLOCK_NUMBER,
        HOMESTEAD_BLOCK_NUMBER, ISTANBUL_BLOCK_NUMBER, LONDON_BLOCK_NUMBER, MERGE_BLOCK_NUMBER,
        SHANGHAI_BLOCK_NUMBER,
    },
};

// tldr:
// Randomly samples X blocks from every hard fork range.
// Validates that each provider is able to return valid
// headers, receipts, and block bodies for each randomly sampled block.
// Tested Providers:
// - Infura
// - PandaOps-Erigon
// - PandaOps-Geth
// - PandaOps-Archive
//
// cargo run --bin test_providers -- --sample-size 5
//

#[tokio::main]
pub async fn main() -> Result<()> {
    init_tracing_logger();
    let config = ProviderConfig::parse();
    let api = ExecutionApi::new(Provider::PandaOps, BridgeMode::Latest)
        .await
        .unwrap();
    let latest_block = api.get_latest_block_number().await?;
    let mut all_ranges = Ranges::into_vec(config.sample_size, latest_block);
    let mut all_providers: Vec<Providers> = Providers::into_vec();
    for provider in all_providers.iter_mut() {
        info!("Testing Provider: {provider}");
        let mut provider_failures = 0;
        let client = provider.get_client();
        let api = ExecutionApi {
            client,
            master_acc: MasterAccumulator::default(),
        };
        for gossip_range in all_ranges.iter_mut() {
            debug!("Testing range: {gossip_range:?}");
            let mut range_failures = 0;
            for block in gossip_range.blocks() {
                debug!("Testing block: {block}");
                let epoch_acc = match lookup_epoch_acc(*block) {
                    Ok(epoch_acc) => epoch_acc,
                    Err(msg) => {
                        provider_failures += 3;
                        range_failures += 3;
                        warn!(
                            "--- failed to build valid header, receipts, & block body for block: {block}: Invalid epoch acc: {msg}"
                        );
                        continue;
                    }
                };
                let (full_header, _, _) = match api.get_header(*block, epoch_acc).await {
                    Ok(header) => header,
                    Err(_) => {
                        provider_failures += 3;
                        range_failures += 3;
                        warn!("--- failed to build valid header, receipts, & block body for block: {block}");
                        continue;
                    }
                };
                if let Err(msg) = api.get_receipts(&full_header).await {
                    provider_failures += 1;
                    range_failures += 1;
                    warn!("--- failed to build valid receipts for block: {block}: Error: {msg}");
                };
                if let Err(msg) = api.get_block_body(&full_header).await {
                    provider_failures += 1;
                    range_failures += 1;
                    warn!("--- failed to build valid block body for block: {block}: Error: {msg}");
                };
            }
            let total = config.sample_size * 3;
            gossip_range.update_success_rate(range_failures, total as u64);
            debug!(
                "Provider: {provider:?} // Range: {gossip_range:?} // Failures: {range_failures}/{total}"
            );
        }
        let total =
            config.sample_size * Ranges::into_vec(config.sample_size, latest_block).len() * 3;
        provider.update_success_rate(provider_failures, total as u64);
        debug!("Provider Summary: {provider:?} // Failures: {provider_failures}/{total}");
    }
    info!("Range Summary:");
    for range in all_ranges.iter() {
        range.display_summary();
    }
    info!("Provider Summary:");
    for provider in all_providers.iter() {
        provider.display_summary();
    }
    info!("Finished testing providers");
    Ok(())
}

fn lookup_epoch_acc(block: u64) -> Result<Option<Arc<EpochAccumulator>>> {
    if block >= MERGE_BLOCK_NUMBER {
        return Ok(None);
    }
    let epoch_index = block / EPOCH_SIZE;
    let master_acc = MasterAccumulator::default();
    let epoch_hash = master_acc.historical_epochs[epoch_index as usize];
    let epoch_hash_pretty = hex_encode(epoch_hash);
    let epoch_hash_pretty = epoch_hash_pretty.trim_start_matches("0x");
    let epoch_acc_path =
        format!("./portal-accumulators/bridge_content/0x03{epoch_hash_pretty}.portalcontent");
    let local_epoch_acc = match fs::read(&epoch_acc_path) {
        Ok(val) => EpochAccumulator::from_ssz_bytes(&val).map_err(|err| anyhow!("{err:?}"))?,
        Err(_) => {
            return Err(anyhow!(
                "Unable to find local epoch acc at path: {epoch_acc_path:?}"
            ))
        }
    };
    Ok(Some(Arc::new(local_epoch_acc)))
}

// CLI Parameter Handling
#[derive(Parser, Debug, PartialEq)]
#[command(
    name = "Provider Config",
    about = "Script to test provider content building stuffs"
)]
pub struct ProviderConfig {
    #[arg(
        long,
        help = "Number of samples to take for each range",
        default_value = "5"
    )]
    pub sample_size: usize,
}

#[derive(Debug, PartialEq)]
enum Ranges {
    // vec of blocks is to store randomly sampled blocks / range
    // so that the same blocks are tested across the providers
    Frontier((Vec<u64>, SuccessRate)),
    Homestead((Vec<u64>, SuccessRate)),
    Byzantium((Vec<u64>, SuccessRate)),
    Constantinople((Vec<u64>, SuccessRate)),
    Istanbul((Vec<u64>, SuccessRate)),
    Berlin((Vec<u64>, SuccessRate)),
    London((Vec<u64>, SuccessRate)),
    Merge((Vec<u64>, SuccessRate)),
    Shanghai((Vec<u64>, SuccessRate)),
}

impl fmt::Display for Ranges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ranges::Frontier(_) => write!(f, "Frontier"),
            Ranges::Homestead(_) => write!(f, "Homestead"),
            Ranges::Byzantium(_) => write!(f, "Byzantium"),
            Ranges::Constantinople(_) => write!(f, "Constantinople"),
            Ranges::Istanbul(_) => write!(f, "Istanbul"),
            Ranges::Berlin(_) => write!(f, "Berlin"),
            Ranges::London(_) => write!(f, "London"),
            Ranges::Merge(_) => write!(f, "Merge"),
            Ranges::Shanghai(_) => write!(f, "Shanghai"),
        }
    }
}

impl Ranges {
    fn display_summary(&self) {
        let success_rate = match self {
            Ranges::Frontier((_, success_rate))
            | Ranges::Homestead((_, success_rate))
            | Ranges::Byzantium((_, success_rate))
            | Ranges::Constantinople((_, success_rate))
            | Ranges::Istanbul((_, success_rate))
            | Ranges::Berlin((_, success_rate))
            | Ranges::London((_, success_rate))
            | Ranges::Merge((_, success_rate))
            | Ranges::Shanghai((_, success_rate)) => success_rate,
        };
        info!(
            "Range: {} // Failure Rate: {}/{}",
            self, success_rate.failures, success_rate.total
        );
    }

    fn blocks(&self) -> &Vec<u64> {
        match self {
            Ranges::Frontier((blocks, _))
            | Ranges::Homestead((blocks, _))
            | Ranges::Byzantium((blocks, _))
            | Ranges::Constantinople((blocks, _))
            | Ranges::Istanbul((blocks, _))
            | Ranges::Berlin((blocks, _))
            | Ranges::London((blocks, _))
            | Ranges::Merge((blocks, _))
            | Ranges::Shanghai((blocks, _)) => blocks,
        }
    }

    fn update_success_rate(&mut self, failures: u64, total: u64) {
        match self {
            Ranges::Frontier((_, success_rate))
            | Ranges::Homestead((_, success_rate))
            | Ranges::Byzantium((_, success_rate))
            | Ranges::Constantinople((_, success_rate))
            | Ranges::Istanbul((_, success_rate))
            | Ranges::Berlin((_, success_rate))
            | Ranges::London((_, success_rate))
            | Ranges::Merge((_, success_rate))
            | Ranges::Shanghai((_, success_rate)) => {
                success_rate.failures += failures;
                success_rate.total += total;
            }
        }
    }

    pub fn into_vec(sample_size: usize, latest_block: u64) -> Vec<Ranges> {
        let mut rng = thread_rng();
        vec![
            Ranges::Frontier((
                Uniform::from(Range {
                    start: 0,
                    end: HOMESTEAD_BLOCK_NUMBER,
                })
                .sample_iter(&mut rng)
                .take(sample_size)
                .collect(),
                SuccessRate::default(),
            )),
            Ranges::Homestead((
                Uniform::from(Range {
                    start: HOMESTEAD_BLOCK_NUMBER,
                    end: BYZANTIUM_BLOCK_NUMBER,
                })
                .sample_iter(&mut rng)
                .take(sample_size)
                .collect(),
                SuccessRate::default(),
            )),
            Ranges::Byzantium((
                Uniform::from(Range {
                    start: BYZANTIUM_BLOCK_NUMBER,
                    end: CONSTANTINOPLE_BLOCK_NUMBER,
                })
                .sample_iter(&mut rng)
                .take(sample_size)
                .collect(),
                SuccessRate::default(),
            )),
            Ranges::Constantinople((
                Uniform::from(Range {
                    start: CONSTANTINOPLE_BLOCK_NUMBER,
                    end: ISTANBUL_BLOCK_NUMBER,
                })
                .sample_iter(&mut rng)
                .take(sample_size)
                .collect(),
                SuccessRate::default(),
            )),
            Ranges::Istanbul((
                Uniform::from(Range {
                    start: ISTANBUL_BLOCK_NUMBER,
                    end: BERLIN_BLOCK_NUMBER,
                })
                .sample_iter(&mut rng)
                .take(sample_size)
                .collect(),
                SuccessRate::default(),
            )),
            Ranges::Berlin((
                Uniform::from(Range {
                    start: BERLIN_BLOCK_NUMBER,
                    end: LONDON_BLOCK_NUMBER,
                })
                .sample_iter(&mut rng)
                .take(sample_size)
                .collect(),
                SuccessRate::default(),
            )),
            Ranges::London((
                Uniform::from(Range {
                    start: LONDON_BLOCK_NUMBER,
                    end: MERGE_BLOCK_NUMBER,
                })
                .sample_iter(&mut rng)
                .take(sample_size)
                .collect(),
                SuccessRate::default(),
            )),
            Ranges::Merge((
                Uniform::from(Range {
                    start: MERGE_BLOCK_NUMBER,
                    end: SHANGHAI_BLOCK_NUMBER,
                })
                .sample_iter(&mut rng)
                .take(sample_size)
                .collect(),
                SuccessRate::default(),
            )),
            Ranges::Shanghai((
                Uniform::from(Range {
                    start: SHANGHAI_BLOCK_NUMBER,
                    end: latest_block,
                })
                .sample_iter(&mut rng)
                .take(sample_size)
                .collect(),
                SuccessRate::default(),
            )),
        ]
    }
}

#[derive(Debug, PartialEq, Default)]
struct SuccessRate {
    failures: u64,
    total: u64,
}

#[derive(Debug, PartialEq)]
enum Providers {
    PandaGeth(SuccessRate),
    PandaErigon(SuccessRate),
    PandaArchive(SuccessRate),
    Infura(SuccessRate),
}

impl fmt::Display for Providers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Providers::PandaGeth(_) => write!(f, "PandaGeth"),
            Providers::PandaErigon(_) => write!(f, "PandaErigon"),
            Providers::PandaArchive(_) => write!(f, "PandaArchive"),
            Providers::Infura(_) => write!(f, "Infura"),
        }
    }
}

impl Providers {
    fn display_summary(&self) {
        let success_rate = match self {
            Providers::PandaGeth(success_rate)
            | Providers::PandaErigon(success_rate)
            | Providers::PandaArchive(success_rate)
            | Providers::Infura(success_rate) => success_rate,
        };
        info!(
            "Provider: {} // Failure Rate: {:?} / {:?}",
            self, success_rate.failures, success_rate.total
        );
    }

    fn update_success_rate(&mut self, failures: u64, total: u64) {
        match self {
            Providers::PandaGeth(success_rate)
            | Providers::PandaErigon(success_rate)
            | Providers::PandaArchive(success_rate)
            | Providers::Infura(success_rate) => {
                success_rate.failures += failures;
                success_rate.total += total;
            }
        }
    }

    fn into_vec() -> Vec<Providers> {
        vec![
            Providers::PandaGeth(SuccessRate::default()),
            Providers::PandaErigon(SuccessRate::default()),
            Providers::PandaArchive(SuccessRate::default()),
            Providers::Infura(SuccessRate::default()),
        ]
    }

    fn get_client(&self) -> Client {
        match self {
            Providers::Infura(_) => {
                let infura_key = std::env::var("TRIN_INFURA_PROJECT_ID").unwrap();
                let base_infura_url =
                    Url::parse(&format!("https://mainnet.infura.io/v3/{}", infura_key)).unwrap();
                Config::new()
                    .add_header("Content-Type", "application/json")
                    .unwrap()
                    .set_base_url(base_infura_url)
                    .try_into()
                    .unwrap()
            }
            _ => {
                let base_el_endpoint = match self {
                    Providers::PandaGeth(_) => {
                        Url::parse("https://geth-lighthouse.mainnet.eu1.ethpandaops.io/")
                            .expect("to be able to parse static base el endpoint url")
                    }
                    Providers::PandaErigon(_) => {
                        Url::parse("https://erigon-lighthouse.mainnet.eu1.ethpandaops.io/")
                            .expect("to be able to parse static base el endpoint url")
                    }
                    Providers::PandaArchive(_) => {
                        Url::parse("https://archive.mainnet.ethpandaops.io/")
                            .expect("to be able to parse static base el endpoint url")
                    }
                    _ => panic!("not implemented"),
                };
                Config::new()
                    .add_header("Content-Type", "application/json")
                    .unwrap()
                    .add_header("CF-Access-Client-Id", PANDAOPS_CLIENT_ID.to_string())
                    .unwrap()
                    .add_header(
                        "CF-Access-Client-Secret",
                        PANDAOPS_CLIENT_SECRET.to_string(),
                    )
                    .unwrap()
                    .set_base_url(base_el_endpoint)
                    .try_into()
                    .unwrap()
            }
        }
    }
}
