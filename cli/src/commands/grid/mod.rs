mod create;
mod redeem;
mod subcommands;

use std::io::Write;

use clap::{Args, Subcommand};
use colored::Colorize;
use ergo_lib::{
    chain::transaction::{unsigned::UnsignedTransaction, TransactionError, UnsignedInput},
    ergotree_ir::{
        chain::ergo_box::{box_value::BoxValue, ErgoBoxCandidate, NonMandatoryRegisters},
        serialization::SigmaParsingError,
    },
    wallet::{box_selector::ErgoBoxAssets, miner_fee::MINERS_FEE_ADDRESS},
};
use off_the_grid::{
    boxes::{
        describe_box::{BoxAssetDisplay, ErgoBoxDescriptors},
        liquidity_box::LiquidityProvider,
        wallet_box::WalletBox,
    },
    grid::multigrid_order::{MultiGridOrder, MultiGridOrderError},
    node::client::NodeClient,
    spectrum::pool::{SpectrumPool, SpectrumSwapError},
    units::{TokenStore, UnitAmount, ERG_UNIT},
};
use tabled::{
    row,
    settings::{
        object::{Columns, Rows},
        Alignment, Disable, Format, Modify, Style,
    },
    Table, Tabled,
};

use crate::scan_config::ScanConfig;

use self::{
    create::{handle_grid_create, CreateOptions},
    redeem::{handle_grid_redeem, RedeemOptions},
    subcommands::{handle_grid_details, handle_grid_list},
};

use super::error::CommandResult;

#[derive(Subcommand)]
pub enum Commands {
    Create(CreateOptions),
    Redeem(RedeemOptions),
    List {
        #[clap(short = 't', long, help = "TokenID to filter by")]
        token_id: Option<String>,
    },
    Details {
        #[clap(short = 'i', long, help = "Grid group identity")]
        grid_identity: String,
    },
}

#[derive(Args)]
pub struct GridCommand {
    #[clap(long, help = "Scan configuration file path [default: scan_config]")]
    scan_config: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

pub async fn handle_grid_command(
    node_client: NodeClient,
    orders_command: GridCommand,
) -> CommandResult<()> {
    let scan_config = ScanConfig::try_create(orders_command.scan_config, None)?;
    let token_store = TokenStore::load(None);
    if token_store.is_err() {
        eprintln!("{}", "Warning: No token configuration found".yellow());

        eprintln!(
            "{}",
            "Run `off-the-grid tokens update` to create one".yellow()
        );
    }
    let token_store = token_store.unwrap_or_default();

    match orders_command.command {
        Commands::Create(options) => {
            let tx = handle_grid_create(&node_client, scan_config, &token_store, options).await?;
            Ok(transaction_query_loop(&node_client, &token_store, tx).await?)
        }
        Commands::Redeem(options) => {
            let data = handle_grid_redeem(&node_client, scan_config, options).await?;
            Ok(transaction_query_loop(&node_client, &token_store, data).await?)
        }
        Commands::List { token_id } => {
            Ok(handle_grid_list(node_client, scan_config, token_id).await?)
        }
        Commands::Details { grid_identity } => {
            Ok(handle_grid_details(node_client, scan_config, grid_identity).await?)
        }
    }
}

async fn transaction_query_loop<T>(
    node_client: &NodeClient,
    token_store: &TokenStore,
    tx_data: T,
) -> anyhow::Result<()>
where
    T: IntoSummarizedTransaction,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    let tx = tx_data.into_summarized_transaction(token_store)?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    let mut line = String::new();

    let table: Table = (&tx).into();

    println!("{}\n", table);

    loop {
        print!("Submit transaction? [Y/n] ");

        stdout.flush()?;
        stdin.read_line(&mut line)?;

        match line.trim() {
            "Y" => {
                let tx = tx.try_into()?;

                let signed = node_client.wallet_transaction_sign(&tx).await?;

                let tx_id = node_client.transaction_submit(&signed).await?;
                println!("Transaction submitted: {}", String::from(tx_id));

                break;
            }
            "n" => {
                println!("Transaction cancelled!");
                break;
            }
            _ => {
                println!("Invalid input, please try again");
                line.clear();
            }
        }
    }

    Ok(())
}

pub trait TryIntoErgoBoxCandidate {
    type Error;

    fn into_ergo_box_candidate(self, creation_height: u32)
        -> Result<ErgoBoxCandidate, Self::Error>;
}

impl<T> TryIntoErgoBoxCandidate for WalletBox<T>
where
    T: ErgoBoxAssets,
{
    type Error = SigmaParsingError;

    fn into_ergo_box_candidate(
        self,
        creation_height: u32,
    ) -> Result<ErgoBoxCandidate, Self::Error> {
        Ok(ErgoBoxCandidate {
            value: self.assets.value(),
            ergo_tree: self.address.script()?,
            tokens: self.assets.tokens(),
            additional_registers: NonMandatoryRegisters::empty(),
            creation_height,
        })
    }
}

impl TryIntoErgoBoxCandidate for MultiGridOrder {
    type Error = MultiGridOrderError;

    fn into_ergo_box_candidate(
        self,
        creation_height: u32,
    ) -> Result<ErgoBoxCandidate, Self::Error> {
        self.into_box_candidate(creation_height)
    }
}

impl TryIntoErgoBoxCandidate for SpectrumPool {
    type Error = SpectrumSwapError;

    fn into_ergo_box_candidate(
        self,
        creation_height: u32,
    ) -> Result<ErgoBoxCandidate, Self::Error> {
        self.into_box_candidate(creation_height)
    }
}

#[derive(Tabled)]
struct BoxSummary {
    #[tabled(rename = "Box type")]
    box_type: String,
    #[tabled(rename = "Value")]
    value: String,
    #[tabled(rename = "Tokens")]
    token: String,
}

impl BoxSummary {
    pub fn new<T: ErgoBoxDescriptors>(desc: &T, token_store: &TokenStore) -> Self {
        let (first_asset, second_asset) = desc.assets(token_store).strings(None);
        Self {
            box_type: desc.box_name(),
            value: first_asset,
            token: second_asset,
        }
    }
}

pub struct SummarizedInput {
    summary: BoxSummary,
    input: UnsignedInput,
}

impl SummarizedInput {
    pub fn new<T: ErgoBoxDescriptors + Into<UnsignedInput>>(
        input: T,
        token_store: &TokenStore,
    ) -> Self {
        let summary = BoxSummary::new(&input, token_store);
        let input = input.into();
        Self { input, summary }
    }
}

pub struct SummarizedOutput {
    output: ErgoBoxCandidate,
    summary: BoxSummary,
}

impl SummarizedOutput {
    pub fn new<T: ErgoBoxDescriptors + TryIntoErgoBoxCandidate>(
        output: T,
        token_store: &TokenStore,
        creation_height: u32,
    ) -> Result<Self, T::Error> {
        let summary = BoxSummary::new(&output, token_store);
        let output = output.into_ergo_box_candidate(creation_height)?;
        Ok(Self { output, summary })
    }
}

fn style_box_table<F>(table: &mut Table, formatting: F)
where
    F: FnMut(&str) -> String + Clone,
{
    table
        .with(Style::empty())
        .with(Modify::new(Columns::new(1..)).with(Alignment::right()))
        .with(Modify::new(Columns::new(0..)).with(Format::content(formatting)))
        .with(Disable::row(Rows::single(0)));
}

/// A transaction with inputs and outputs that also contain a summary of the
/// invididual inputs and outputs.
pub(super) struct SummarizedTransaction {
    pub inputs: Vec<SummarizedInput>,
    pub outputs: Vec<SummarizedOutput>,
}

trait IntoSummarizedTransaction {
    type Error;

    fn into_summarized_transaction(
        self,
        token_store: &TokenStore,
    ) -> Result<SummarizedTransaction, Self::Error>;
}

impl From<&'_ SummarizedTransaction> for Table {
    fn from(value: &SummarizedTransaction) -> Self {
        let input_descriptions = value.inputs.iter().map(|input| &input.summary);

        let output_descriptions = value.outputs.iter().map(|output| &output.summary);

        let mut input = Table::new(input_descriptions);
        style_box_table(&mut input, |i| i.bright_red().to_string());

        let mut output = Table::new(output_descriptions);
        style_box_table(&mut output, |i| i.bright_green().to_string());

        let mut combined = row![input, output];
        combined.with(Style::empty());
        combined
    }
}

impl TryFrom<SummarizedTransaction> for UnsignedTransaction {
    type Error = TransactionError;

    fn try_from(value: SummarizedTransaction) -> Result<Self, Self::Error> {
        let inputs = value.inputs.into_iter().map(|input| input.input).collect();

        let outputs = value
            .outputs
            .into_iter()
            .map(|output| output.output)
            .collect();

        UnsignedTransaction::new_from_vec(inputs, vec![], outputs)
    }
}

/// Wrapper over a box value to describe it as a miner fee
struct MinerFeeValue(pub BoxValue);

impl ErgoBoxDescriptors for MinerFeeValue {
    fn box_name(&self) -> String {
        "Miner fee".to_string()
    }

    fn assets<'a>(&self, _: &'a TokenStore) -> BoxAssetDisplay<'a> {
        let amount = UnitAmount::new(*ERG_UNIT, *self.0.as_u64());
        BoxAssetDisplay::Single(amount)
    }
}

impl TryIntoErgoBoxCandidate for MinerFeeValue {
    type Error = ();

    fn into_ergo_box_candidate(
        self,
        creation_height: u32,
    ) -> Result<ErgoBoxCandidate, Self::Error> {
        Ok(ErgoBoxCandidate {
            value: self.0,
            ergo_tree: MINERS_FEE_ADDRESS
                .script()
                .expect("Miner fee is predefined"),
            tokens: None,
            additional_registers: NonMandatoryRegisters::empty(),
            creation_height,
        })
    }
}
