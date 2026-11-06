//! ai-cu の入口。振り分けは `ai_cu::cli` にあり、ここは実際の環境 (環境変数、既定の場所、装置) をつなぐだけ。

use std::io::{IsTerminal, Write};
use std::sync::Arc;

use clap::Parser;

use ai_cu::cli::{Cli, Context, Outcome, dispatch};
use ai_cu::config::{ProcessEnv, SystemLocator};
use ai_cu::error::{AppError, ErrorKind};
use ai_cu::image::PngFileWriter;
use ai_cu::mcp::{Inbox, Server, host_unknown_refusal, serve};
use ai_cu::runner::{RealDevices, RealRunner};

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    // 前面化の補助プロセス (D-057)。MCP の focus が起動する。設定もガードも読まず、前面化を 1 回だけ行う。
    // clap の外で見分けるので、--help と schema には出ない
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if let Some(code) = ai_cu::focus_helper::real_main_if_helper(&args) {
        return code;
    }
    // 座標を物理ピクセルにそろえる。最初のウィンドウや座標の取得より前に宣言する。
    // 失敗したら、座標を扱うコマンド (run と shot) は dispatch が拒否する
    let dpi = ai_cu::win32::dpi::ensure_per_monitor_v2();
    if let Err(e) = &dpi {
        let _ = writeln!(std::io::stderr(), "ai-cu: 警告: {e}");
    }
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let code = e.exit_code();
            // --help と --version は標準出力へ、誤りは標準エラーへ (clap の既定)
            let _ = e.print();
            if code != 0 {
                // 取り違えの事例には専用の hint を付ける (D-086)。人が読む標準エラーにも 1 行足す
                let err = ai_cu::cli::usage_error(&args, e.kind(), e.render().to_string());
                if let Some(h) = &err.hint {
                    let _ = writeln!(std::io::stderr(), "ai-cu: ヒント: {h}");
                }
                println!("{}", err.to_json());
                return ErrorKind::Usage.exit_code();
            }
            return 0;
        }
    };
    // mintty の pty は IsTerminal では端末に見えるが、crossterm はキーを読めない。本物のコンソールに限る
    let stdin_tty = std::io::stdin().is_terminal() && ai_cu::win32::console::stdin_is_console();
    let stdout_tty = std::io::stdout().is_terminal() && ai_cu::win32::console::stdout_is_console();
    // ai-cu を起動した側 (親プロセスの連鎖) のウィンドウは、設定にかかわらず対象にしない (D-055)
    let host = match ai_cu::win32::process::host_chain() {
        Ok(h) => h,
        Err(e) => {
            let _ = writeln!(
                std::io::stderr(),
                "ai-cu: 警告: 起動した側のプロセスを特定できない ({e})。MCP では実入力を断る"
            );
            Vec::new()
        }
    };
    let host_pids: Vec<u32> = host.iter().map(|h| h.pid).collect();
    let mut stdin = std::io::stdin();
    let mut devices = RealDevices::new();
    let mut images = PngFileWriter;
    let mut ctx = Context {
        env: &ProcessEnv,
        locator: &SystemLocator,
        stdin: &mut stdin,
        stdin_tty,
        stdout_tty,
        devices: &mut devices,
        images: &mut images,
        dpi: dpi.clone(),
        host_pids: host_pids.clone(),
        surface: ai_cu::hint::Surface::Cli,
        stops: Vec::new(),
    };
    match dispatch(cli, &mut ctx) {
        Outcome::Json {
            code,
            value,
            stderr,
        } => {
            if let Some(s) = stderr {
                eprintln!("{s}");
            }
            println!("{value}");
            code
        }
        Outcome::Tui { target, paths } => {
            // TUI からの実行は、CLI の run と同じ装置と層を通す (D-044)
            let mut runner = RealRunner {
                paths,
                env: &ProcessEnv,
                devices: &mut devices,
                images: &mut images,
                dpi,
                host_pids,
            };
            ai_cu::tui::run(&target, &mut runner)
        }
        Outcome::Text(text) => {
            print!("{text}");
            0
        }
        Outcome::Mcp { paths, log } => {
            // MCP サーバー (D-055)。標準出力には MCP のメッセージ以外を書かない
            if let Some(path) = log
                && let Err(e) = ai_cu::mcp::set_log_file(&path)
            {
                // 診断のための機能なので、開けなくても道具は止めない (D-061)
                ai_cu::mcp::log(&format!(
                    "ログのファイル {} を開けない ({e})。ログなしで続ける",
                    path.display()
                ));
            }
            let hosts: Vec<String> = host
                .iter()
                .map(|h| format!("{} ({})", h.exe, h.pid))
                .collect();
            ai_cu::mcp::log(&format!(
                "{} を起動。JSON-RPC を標準入力から読む。host: [{}]",
                ai_cu::build_info::VERSION,
                hosts.join(", ")
            ));
            // 枠を出せない警告と入力の錠を解けない警告も MCP の記録 (標準エラーと --log のファイル) に載せる
            // (D-093、D-061、D-113)
            devices.set_warn_sink(ai_cu::mcp::log);
            let inbox = Arc::new(Inbox::default());
            let mut server = Server::new(
                paths,
                &ProcessEnv,
                &SystemLocator,
                &mut devices,
                &mut images,
                dpi,
                host_pids,
                inbox,
            );
            // 検証前の debug ビルドが MCP の経路 (hook に見えない) から実入力を送らないよう、release だけが送れる。
            // ホストを特定できないときも実入力は送らない (fail-closed)
            if cfg!(debug_assertions) {
                server.refuse_input = Some(AppError::new(
                    ErrorKind::ForbiddenDevEnv,
                    "debug ビルドの ai-cu mcp は実入力を送らない (検証を通した release ビルドを登録する。D-055)",
                ));
            } else if host.is_empty() {
                // focus や再試行では変わらないので、専用の理由と次の手を付ける (D-056)
                server.refuse_input = Some(host_unknown_refusal());
            }
            if let Some(e) = &server.refuse_input {
                let _ = writeln!(
                    std::io::stderr(),
                    "ai-cu mcp: 実入力の道具は断る: {}",
                    e.message
                );
            }
            let stdout: Box<dyn std::io::Write + Send> = Box::new(std::io::stdout());
            serve(
                &mut server,
                std::io::BufReader::new(std::io::stdin()),
                stdout,
            )
        }
    }
}
