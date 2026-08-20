//! # quilt-jetson CLI
//!
//! The `quilt-jetson` command-line entry point. Loads a sheet, starts
//! the web server, and (optionally) wires up the ROS2 bridge and the
//! federation client.
//!
//! ## Subcommands
//!
//! - `serve <sheet.yaml>` — start the engine + web server.
//! - `eval <sheet.yaml> <cell>` — evaluate a single cell and print
//!   the result.
//! - `meta <sheet.yaml>` — print engine metadata.
//! - `validate <sheet.yaml>` — validate a sheet without loading it.
//!
//! ## Usage
//!
//! ```text
//! $ quilt-jetson serve examples/sensor-fusion.yaml --port 8080
//! $ quilt-jetson eval examples/vision-detect.yaml vision.obstacles
//! $ quilt-jetson validate examples/ros2-publisher.yaml
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::EnvFilter;

use quilt_jetson::{
    parse_sheet, parse_sheet_file, EngineConfig, Error, QuiltEngine, SqliteStore,
};

#[derive(Parser, Debug)]
#[command(name = "quilt-jetson")]
#[command(about = "Quilt reactive runtime for NVIDIA Jetson devices")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start the engine + web server.
    Serve {
        /// Path to the sheet YAML file.
        sheet: PathBuf,
        /// The port to listen on. Default 8080.
        #[arg(long, default_value = "8080")]
        port: u16,
        /// The address to bind to. Default 0.0.0.0.
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        /// Path to the SQLite database for cell history.
        #[arg(long)]
        store: Option<PathBuf>,
        /// Enable tracing.
        #[arg(long)]
        trace: bool,
    },
    /// Evaluate a single cell and print the result.
    Eval {
        /// Path to the sheet YAML file.
        sheet: PathBuf,
        /// The cell id to evaluate.
        cell: String,
    },
    /// Print engine metadata.
    Meta {
        /// Path to the sheet YAML file.
        sheet: PathBuf,
    },
    /// Validate a sheet YAML file.
    Validate {
        /// Path to the sheet YAML file.
        sheet: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move { run(cli).await })
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Serve {
            sheet,
            port,
            bind,
            store,
            trace,
        } => {
            serve(sheet, bind, port, store, trace).await?;
        }
        Command::Eval { sheet, cell } => {
            eval_cell(sheet, cell).await?;
        }
        Command::Meta { sheet } => {
            print_meta(sheet).await?;
        }
        Command::Validate { sheet } => {
            validate_sheet(sheet)?;
        }
    }
    Ok(())
}

async fn serve(
    sheet: PathBuf,
    bind: String,
    port: u16,
    store_path: Option<PathBuf>,
    trace: bool,
) -> anyhow::Result<()> {
    let sheet_def = parse_sheet_file(&sheet)
        .map_err(|e| anyhow::anyhow!("could not load sheet {}: {e}", sheet.display()))?;
    info!("loaded sheet: {} ({} cells)", sheet_def.id, sheet_def.cells.len());

    let config = EngineConfig {
        tracing: trace,
        store: if let Some(p) = store_path {
            let s = SqliteStore::open(&p).await?;
            Some(Arc::new(s))
        } else {
            None
        },
        ..Default::default()
    };

    let engine = QuiltEngine::with_sheet(
        sheet_def.id.clone(),
        config,
        sheet_def,
    )?;
    info!("engine ready: id={} cells={}", engine.id(), engine.list_ids().len());

    let addr: std::net::SocketAddr = format!("{bind}:{port}").parse()?;
    quilt_jetson::web::serve(engine, quilt_jetson::web::WebConfig { bind: addr }).await?;
    Ok(())
}

async fn eval_cell(sheet: PathBuf, cell: String) -> anyhow::Result<()> {
    let sheet_def = parse_sheet_file(&sheet)?;
    let engine = QuiltEngine::with_sheet(
        sheet_def.id.clone(),
        EngineConfig::default(),
        sheet_def,
    )?;
    let value = engine
        .get(&cell, quilt_jetson::CallerContext::default())
        .await?;
    let json = serde_json::to_string_pretty(&value)?;
    println!("{json}");
    Ok(())
}

async fn print_meta(sheet: PathBuf) -> anyhow::Result<()> {
    let sheet_def = parse_sheet_file(&sheet)?;
    let engine = QuiltEngine::with_sheet(
        sheet_def.id.clone(),
        EngineConfig::default(),
        sheet_def,
    )?;
    let meta = serde_json::json!({
        "id": engine.id(),
        "cell_count": engine.list_ids().len(),
        "cells": engine.list_ids(),
    });
    println!("{}", serde_json::to_string_pretty(&meta)?);
    Ok(())
}

fn validate_sheet(sheet: PathBuf) -> anyhow::Result<()> {
    let _ = parse_sheet_file(&sheet).map_err(|e: Error| anyhow::anyhow!("{e}"))?;
    println!("✓ sheet is valid: {}", sheet.display());
    Ok(())
}

#[allow(dead_code)]
fn _unused_parse() {
    let _ = parse_sheet;
}
