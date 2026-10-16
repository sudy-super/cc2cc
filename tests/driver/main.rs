use std::path::PathBuf;

use cc2cc::bridge::{ConvertOptions, InstallOptions, convert, install, verify};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "core-driver",
    version,
    about = "Claude CodeとClawCodexの会話履歴を原本付きで相互変換します"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 生履歴またはcc2cc出力を変換する
    Convert {
        source: PathBuf,
        #[arg(long = "from", value_parser = ["claude", "clawcodex"])]
        source_format: Option<String>,
        #[arg(long = "to", value_parser = ["claude", "clawcodex"])]
        target: String,
        #[arg(short, long)]
        output: PathBuf,
        /// Claude Codeで変換する分岐の末端UUID
        #[arg(long)]
        leaf: Option<String>,
        /// 移行前のcc2cc出力ディレクトリ
        #[arg(long)]
        provenance: Option<PathBuf>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
        /// ClawCodexの子履歴ディレクトリ
        #[arg(long)]
        transcripts_dir: Option<PathBuf>,
        /// 原本に保存する追加ファイル/ディレクトリ (複数指定可)
        #[arg(long)]
        attachments: Vec<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    /// 原本ハッシュと履歴の整合性を検証する
    Verify { bundle: PathBuf },
    /// 新しいIDでネイティブ履歴へ登録する
    Install {
        bundle: PathBuf,
        #[arg(long)]
        home: Option<PathBuf>,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() {
    let result = match Cli::parse().command {
        Command::Convert {
            source,
            source_format,
            target,
            output,
            leaf,
            provenance,
            cwd,
            session_id,
            transcripts_dir,
            attachments,
            dry_run,
        } => convert(
            &source,
            &target,
            &output,
            &ConvertOptions {
                source_format,
                leaf,
                provenance,
                cwd,
                session_id,
                transcripts_dir,
                attachments,
                dry_run,
            },
        ),
        Command::Verify { bundle } => verify(&bundle),
        Command::Install {
            bundle,
            home,
            session_id,
            dry_run,
        } => install(
            &bundle,
            &InstallOptions {
                home,
                session_id,
                dry_run,
            },
        ),
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
