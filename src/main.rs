mod arg_parser;
mod args;
mod thin_client;

use crate::arg_parser::parse_args;
use crate::args::{resolve_command, Command, DistributeArgs};
use crate::thin_client::{NetworkClient, ThinClient};
use console::style;
use csv::{ReaderBuilder, Trim};
use serde::{Deserialize, Serialize};
use solana_cli_config::Config;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    message::Message,
    native_token::sol_to_lamports,
    signature::{Signature, Signer},
    system_instruction,
};
use std::env;
use std::error::Error;
use std::fs;
use std::path::Path;

#[derive(Deserialize, Debug, Clone)]
struct Bid {
    bid_amount_dollars: f64,
    primary_address: String,
}

struct Allocation {
    recipient: String,
    amount: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TransactionInfo {
    recipient: String,
    amount: f64,
    signature: String,
}

fn apply_previous_transactions(
    allocations: &mut Vec<Allocation>,
    transaction_infos: &[TransactionInfo],
) {
    for transaction_info in transaction_infos {
        let mut amount = transaction_info.amount;
        for allocation in allocations.iter_mut() {
            if allocation.amount >= amount {
                allocation.amount -= amount;
                break;
            } else {
                amount -= allocation.amount;
                allocation.amount = 0.0;
            }
        }
    }
    allocations.retain(|x| x.amount > 0.0);
}

fn create_allocation(bid: &Bid, dollars_per_sol: f64) -> Allocation {
    Allocation {
        recipient: bid.primary_address.clone(),
        amount: bid.bid_amount_dollars / dollars_per_sol,
    }
}
fn distribute_tokens<T: NetworkClient>(
    client: &ThinClient<T>,
    allocations: &[Allocation],
    args: &DistributeArgs<Box<dyn Signer>>,
) -> Vec<Signature> {
    let messages: Vec<Message> = allocations
        .iter()
        .map(|allocation| {
            let from = args.sender_keypair.as_ref().unwrap().pubkey();
            let to = allocation.recipient.parse().unwrap();
            let lamports = sol_to_lamports(allocation.amount);
            let instruction = system_instruction::transfer(&from, &to, lamports);
            Message::new(&[instruction])
        })
        .collect();

    let signers = vec![
        &**args.sender_keypair.as_ref().unwrap(),
        &**args.fee_payer.as_ref().unwrap(),
    ];

    messages
        .into_iter()
        .map(|message| client.send_message(message, &signers).unwrap())
        .collect()
}

fn append_transaction_infos(
    allocations: &[Allocation],
    signatures: &[Signature],
    transactions_csv: &str,
) -> Result<(), csv::Error> {
    let existed = Path::new(&transactions_csv).exists();
    if existed {
        let transactions_bak = format!("{}.bak", &transactions_csv);
        fs::copy(&transactions_csv, transactions_bak)?;
    }
    let file = fs::OpenOptions::new()
        .create_new(!existed)
        .write(true)
        .append(existed)
        .open(&transactions_csv)?;
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(!existed)
        .from_writer(file);

    for (i, allocation) in allocations.iter().enumerate() {
        let transaction_info = TransactionInfo {
            recipient: allocation.recipient.clone(),
            amount: allocation.amount,
            signature: signatures[i].to_string(),
        };
        wtr.serialize(transaction_info)?;
    }
    wtr.flush()?;
    Ok(())
}

fn process_distribute<T: NetworkClient>(
    client: &ThinClient<T>,
    args: &DistributeArgs<Box<dyn Signer>>,
) -> Result<(), csv::Error> {
    let mut rdr = ReaderBuilder::new()
        .trim(Trim::All)
        .from_path(&args.allocations_csv)?;
    let mut allocations: Vec<Allocation> = rdr
        .deserialize()
        .map(|bid| create_allocation(&bid.unwrap(), args.dollars_per_sol))
        .collect();

    let transaction_infos: Vec<TransactionInfo> = if Path::new(&args.transactions_csv).exists() {
        let mut state_rdr = ReaderBuilder::new()
            .trim(Trim::All)
            .from_path(&args.transactions_csv)?;
        state_rdr.deserialize().map(|x| x.unwrap()).collect()
    } else {
        vec![]
    };
    apply_previous_transactions(&mut allocations, &transaction_infos);

    if allocations.is_empty() {
        eprintln!("No work to do");
        return Ok(());
    }

    println!(
        "{}",
        style(format!("{:<44}  {}", "Recipient", "Amount")).bold()
    );
    for allocation in &allocations {
        println!("{:<44}  {}", allocation.recipient, allocation.amount);
    }

    if !args.dry_run {
        let signatures = distribute_tokens(&client, &allocations, &args);
        append_transaction_infos(&allocations, &signatures, &args.transactions_csv)?;
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let command_args = parse_args(env::args_os());
    let config = Config::load(&command_args.config_file)?;
    let json_rpc_url = command_args.url.unwrap_or(config.json_rpc_url);
    let rpc_client = RpcClient::new(json_rpc_url);
    let client = ThinClient(rpc_client);

    match resolve_command(&command_args.command)? {
        Command::Distribute(args) => {
            process_distribute(&client, &args)?;
        }
    }
    Ok(())
}
