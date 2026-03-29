# Cli

## Testing

You can test your changes to the `cli` crate by first building the main quorp binary:

```
cargo build -p quorp
```

And then building and running the `cli` crate with the following parameters:

```
 cargo run -p cli -- --quorp ./target/debug/quorp.exe
```
