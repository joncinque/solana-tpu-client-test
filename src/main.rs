use {
    clap::{crate_description, crate_name, crate_version, Arg, Command},
    solana_clap_v3_utils::{
        input_validators::{is_url_or_moniker, is_valid_signer, normalize_to_url_if_moniker},
        keypair::DefaultSigner,
    },
    solana_client::{
        connection_cache::ConnectionCache,
        nonblocking::tpu_client::TpuClient,
        send_and_confirm_transactions_in_parallel::{
            send_and_confirm_transactions_in_parallel, SendAndConfirmConfig,
        },
        tpu_client::TpuClientConfig,
    },
    solana_remote_wallet::remote_wallet::RemoteWalletManager,
    solana_rpc_client::nonblocking::rpc_client::RpcClient,
    solana_sdk::{
        commitment_config::CommitmentConfig, compute_budget::ComputeBudgetInstruction,
        message::Message, signature::Signer, system_instruction,
    },
    std::{process::exit, rc::Rc, sync::Arc, time::Instant},
};

struct Config {
    default_signer: Box<dyn Signer>,
    json_rpc_url: String,
    websocket_url: String,
}

async fn process_ping(
    rpc_client: RpcClient,
    websocket_url: &str,
    signer: &dyn Signer,
    num_messages: u64,
    use_rpc: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let from = signer.pubkey();
    let to = signer.pubkey();
    let blockhash = rpc_client.get_latest_blockhash().await?;
    let messages = (0..num_messages)
        .map(|i| {
            Message::new_with_blockhash(
                &[
                    ComputeBudgetInstruction::set_compute_unit_price(1_000_000),
                    ComputeBudgetInstruction::set_compute_unit_limit(450),
                    system_instruction::transfer(&from, &to, i),
                ],
                Some(&signer.pubkey()),
                &blockhash,
            )
        })
        .collect::<Vec<_>>();

    let now = Instant::now();
    let connection_cache = ConnectionCache::new_quic("connection_cache_cli_program_quic", 1);
    let rpc_client = Arc::new(rpc_client);
    let transaction_errors = if let ConnectionCache::Quic(cache) = connection_cache {
        let tpu_client = (!use_rpc).then_some(
            TpuClient::new_with_connection_cache(
                rpc_client.clone(),
                websocket_url,
                TpuClientConfig::default(),
                cache,
            )
            .await?,
        );
        send_and_confirm_transactions_in_parallel(
            rpc_client,
            tpu_client,
            &messages,
            &[signer],
            SendAndConfirmConfig {
                resign_txs_count: Some(5),
                with_spinner: true,
            },
        )
        .await
        /*
        tpu_client
            .send_and_confirm_messages_with_spinner(&messages, &[signer])
            .await
        */
        .map_err(|err| format!("Data writes to account failed: {err}"))?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
    } else {
        panic!("not possible");
    };

    if !transaction_errors.is_empty() {
        for transaction_error in &transaction_errors {
            println!("{:?}", transaction_error);
        }
        return Err(format!("{} write transactions failed", transaction_errors.len()).into());
    }

    println!("Took {}ms", now.elapsed().as_millis());
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app_matches = Command::new(crate_name!())
        .about(crate_description!())
        .version(crate_version!())
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg({
            let arg = Arg::new("config_file")
                .short('C')
                .long("config")
                .value_name("PATH")
                .takes_value(true)
                .global(true)
                .help("Configuration file to use");
            if let Some(ref config_file) = *solana_cli_config::CONFIG_FILE {
                arg.default_value(config_file)
            } else {
                arg
            }
        })
        .arg(
            Arg::new("keypair")
                .long("keypair")
                .value_name("KEYPAIR")
                .validator(|s| is_valid_signer(s))
                .takes_value(true)
                .global(true)
                .help("Filepath or URL to a keypair [default: client keypair]"),
        )
        .arg(
            Arg::new("json_rpc_url")
                .short('u')
                .long("url")
                .value_name("URL")
                .takes_value(true)
                .global(true)
                .validator(|s| is_url_or_moniker(s))
                .help("JSON RPC URL for the cluster [default: value from configuration file]"),
        )
        .subcommand(
            Command::new("ping")
                .about("Send a ping transaction")
                .arg(
                    Arg::new("num_messages")
                        .value_parser(clap::value_parser!(u64))
                        .value_name("NUMBER_OF_MESSAGES")
                        .takes_value(true)
                        .index(1)
                        .required(true)
                        .help("The number of messages to send"),
                )
                .arg(
                    Arg::new("use_rpc")
                        .long("use-rpc")
                        .takes_value(false)
                        .help("Send transactions over RPC instead of TPU"),
                ),
        )
        .get_matches();

    let (command, matches) = app_matches.subcommand().unwrap();
    let mut wallet_manager: Option<Rc<RemoteWalletManager>> = None;

    let config = {
        let cli_config = if let Some(config_file) = matches.value_of("config_file") {
            solana_cli_config::Config::load(config_file).unwrap_or_default()
        } else {
            solana_cli_config::Config::default()
        };

        let default_signer = DefaultSigner::new(
            "keypair",
            matches
                .value_of("keypair")
                .map(|s| s.to_string())
                .unwrap_or_else(|| cli_config.keypair_path.clone()),
        );

        let json_rpc_url = normalize_to_url_if_moniker(
            matches
                .value_of("json_rpc_url")
                .unwrap_or(&cli_config.json_rpc_url),
        );

        let websocket_url = solana_cli_config::Config::compute_websocket_url(&json_rpc_url);
        Config {
            default_signer: default_signer
                .signer_from_path(matches, &mut wallet_manager)
                .unwrap_or_else(|err| {
                    eprintln!("error: {err}");
                    exit(1);
                }),
            json_rpc_url,
            websocket_url,
        }
    };
    solana_logger::setup_with_default("solana=info");

    let rpc_client =
        RpcClient::new_with_commitment(config.json_rpc_url.clone(), CommitmentConfig::confirmed());

    match (command, matches) {
        ("ping", arg_matches) => {
            let num_messages = arg_matches.try_get_one::<u64>("num_messages")?.unwrap();
            process_ping(
                rpc_client,
                &config.websocket_url,
                config.default_signer.as_ref(),
                *num_messages,
                arg_matches.try_contains_id("use_rpc")?,
            )
            .await
            .unwrap_or_else(|err| {
                eprintln!("error: send transaction: {err}");
                exit(1);
            });
        }
        _ => unreachable!(),
    };

    Ok(())
}
