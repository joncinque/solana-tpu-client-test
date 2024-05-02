# solana-tpu-client-test

Test CLI for landing Solana transactions over TPU (or RPC). Uses your locally
configured keypair for sending self-transfers. Also includes priority fees by
default to help land the transaction more easily.

Crucially, this tool uses crates from a local checkout of the Agave validator,
assumed to exist at `../solana`. See the Cargo.toml file to customize for your
own usage. By using local crates, it's easy to test out any changes to clients
on any network.

## Examples

* Send 100 self-transfers on testnet over RPC:

```
cargo run -- --ut ping 100 --use-rpc
```

* Send 10 self-transfers on mainnet over TPU:

```
cargo run -- -um ping 100
```
