# Example CLI for `emote-psb-rs`

This ZIP is structured to be unpacked at the root of `storycraft/emote-psb-rs`.
It adds an example binary target named `emote-psb` and a GitHub Actions workflow
that builds that example as a release binary and uploads it as an artifact.

## Files added or changed

```text
Cargo.toml
examples/emote_psb_cli.rs
.github/workflows/build-example-cli.yml
README.cli-example.md
```

`Cargo.toml` is the upstream manifest with the following additions:

```toml
[dev-dependencies]
anyhow = "1"
clap = { version = "4.5", features = ["derive"] }
serde_json = "1"

[[example]]
name = "emote-psb"
path = "examples/emote_psb_cli.rs"
```

## Local build

```bash
cargo build --release --example emote-psb
```

The built binary is:

```text
target/release/examples/emote-psb
```

## GitHub Actions build

The workflow builds:

```bash
cargo build --release --example emote-psb --target x86_64-unknown-linux-gnu
```

and uploads:

```text
dist/emote-psb-x86_64-unknown-linux-gnu
dist/emote-psb-x86_64-unknown-linux-gnu.sha256
```

## CLI commands

```bash
cargo run --example emote-psb -- info model.psb
cargo run --example emote-psb -- info model.mdf --json
cargo run --example emote-psb -- dump model.psb -o root.json --pretty
cargo run --example emote-psb -- unpack model.mdf out --overwrite
cargo run --example emote-psb -- pack out/manifest.json rebuilt.psb
cargo run --example emote-psb -- build root.json rebuilt.psb --version 4 --resource res0.bin --extra extra0.bin
cargo run --example emote-psb -- repack old.mdf normalized.psb
cargo run --example emote-psb -- encode-mdf model.psb model.mdf --level 6
cargo run --example emote-psb -- decode-mdf model.mdf model.psb
cargo run --example emote-psb -- convert model.psb model.mdf
cargo run --example emote-psb -- convert model.mdf model.psb
```

After building, use the binary directly:

```bash
./target/release/examples/emote-psb info model.psb
```
