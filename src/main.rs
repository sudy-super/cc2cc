use cc2cc::sessions;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Claude CodeとClawCodexの会話履歴を変換します")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 保存されているセッションを一覧表示する
    List {
        #[arg(long = "from", value_parser = ["claude", "clawcodex"])]
        tool: Option<String>,
    },
    /// セッションIDを指定して変換し、履歴に登録する
    Convert {
        session_id: String,
        #[arg(long = "to", value_parser = ["claude", "clawcodex"])]
        target: String,
        /// 複数の分岐がある場合の末端UUID
        #[arg(long)]
        leaf: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// 変換したセッションと保存した原本を検証する
    Verify {
        session_id: String,
        #[arg(long = "from", value_parser = ["claude", "clawcodex"])]
        tool: Option<String>,
    },
}

fn main() {
    let result = match Cli::parse().command {
        Command::List { tool } => sessions::list(tool.as_deref()),
        Command::Convert {
            session_id,
            target,
            leaf,
            dry_run,
        } => sessions::convert_id(&session_id, &target, leaf, dry_run),
        Command::Verify { session_id, tool } => sessions::verify_id(&session_id, tool.as_deref()),
    };
    match result {
        Ok(report) => println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("JSON report")
        ),
        Err(error) => {
            eprintln!("cc2cc: {error:#}");
            std::process::exit(2);
        }
    }
}
