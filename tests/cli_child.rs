//! 実際のバイナリを子プロセスで起動する結合テスト。実入力を送らない起動 (引数なし、読み取り専用、--dry-run) だけを行う。
//! 子プロセスには開発シェルの変数 (`AI_CU_DEV_FORBID_REAL_INPUT`) を明示的に付け、利用者の実際の設定を読ませない (D-010、D-022)。

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_ai-cu");
const TIMEOUT: Duration = Duration::from_secs(10);

/// `target/test-tmp/child-<名前>-<プロセス ID>-<時刻>`。古い場所の掃除もライブラリの側が行う (D-087)
fn temp_dir(name: &str) -> PathBuf {
    ai_cu::testing::unique_temp_dir(&format!("child-{name}"))
}

/// 標準入力、標準出力、標準エラーをすべてパイプにした、開発シェルの子プロセス。
fn command(args: &[&str]) -> Command {
    let mut c = Command::new(BIN);
    c.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("AI_CU_DEV_FORBID_REAL_INPUT", "1")
        .env_remove("AI_CU_CONFIG")
        .env_remove("AI_CU_DEV_FLOOR")
        .env_remove("AI_CU_NO_TUI");
    c
}

/// 期限内に終わるのを待つ。期限を過ぎたら kill して失敗にする (ハングの検出)。
/// 標準出力と標準エラーは別スレッドで読み続ける (読まないとパイプが詰まって子が書き込みで止まる。MCP の
/// 応答は数十 KB になる)。標準入力は全部書いてから閉じる (EOF)。
fn wait_with_timeout(mut child: Child, stdin_data: &[u8]) -> (Output, Duration) {
    let start = Instant::now();
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    );
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_data);
        // ここで閉じる (EOF)
    }
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => break s,
            None if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "{:?} 以内に終わらなかった (ハング)。stdout={} stderr={}",
                    TIMEOUT,
                    String::from_utf8_lossy(&stdout.join().unwrap_or_default()),
                    String::from_utf8_lossy(&stderr.join().unwrap_or_default())
                );
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let elapsed = start.elapsed();
    (
        Output {
            status,
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        },
        elapsed,
    )
}

fn run(args: &[&str], envs: &[(&str, &str)], stdin: &[u8]) -> (i32, String, String) {
    let mut c = command(args);
    for (k, v) in envs {
        c.env(k, v);
    }
    let (out, _) = wait_with_timeout(c.spawn().expect("spawn"), stdin);
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn tty_child_no_args_with_piped_stdio_prints_usage_and_exits_nonzero_within_10s() {
    let (out, elapsed) = wait_with_timeout(command(&[]).spawn().expect("spawn"), b"");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    println!(
        "elapsed={elapsed:?} code={:?}\nstdout={stdout}\nstderr={stderr}",
        out.status.code()
    );
    assert!(elapsed < TIMEOUT);
    assert_eq!(out.status.code(), Some(2), "使い方の誤りとして 2 で終わる");
    assert!(
        stderr.contains("Usage:"),
        "usage を標準エラーに出す: {stderr}"
    );
    assert!(
        stdout.contains("\"code\":\"usage\""),
        "標準出力は機械可読な JSON: {stdout}"
    );
}

#[test]
fn tty_child_no_args_with_r_on_stdin_never_reaches_tui_run() {
    // TUI からの実行 (r キー、D-043) は、端末の無い起動からは到達できない。標準入力に r を流しても usage で 2
    let (code, stdout, stderr) = run(&[], &[], b"r\ny\n");
    assert_eq!(code, 2, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("\"code\":\"usage\""), "{stdout}");
    assert!(!stderr.contains("実行"), "{stderr}");
}

#[test]
fn tty_child_no_args_with_config_still_refuses_without_terminal() {
    let (code, _, stderr) = run(&["--config=target/never-created.toml"], &[], b"");
    assert_eq!(code, 2);
    assert!(stderr.contains("Usage:"), "{stderr}");
}

#[test]
fn validate_child_schema_is_json_and_lists_actions() {
    let (code, stdout, _) = run(&["schema"], &[], b"");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(v["version"], 1);
    assert!(v["actions"]["drag"].is_object());
}

#[test]
fn validate_child_reads_stdin_and_reports_policy_denied_in_dev_shell() {
    let script = br#"{"version":1,"actions":[{"type":"click","x":10,"y":10}]}"#;
    let (code, stdout, _) = run(&["validate", "-"], &[], script);
    assert_eq!(code, 4, "{stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(v["error"]["code"], "policy_denied");
}

#[test]
fn validate_child_rejects_unknown_fields_with_exit_3() {
    let script = br#"{"version":1,"actions":[{"type":"click","x":10,"y":10,"force":true}]}"#;
    let (code, stdout, _) = run(&["validate", "-"], &[], script);
    assert_eq!(code, 3, "{stdout}");
}

#[test]
fn dry_run_child_outputs_plan_with_dev_floor() {
    let dir = temp_dir("dry");
    let floor = dir.join("floor.toml");
    std::fs::write(&floor, "version = 1\n[general]\ninput_enabled = true\n").unwrap();
    let script_path = dir.join("script.json");
    std::fs::write(
        &script_path,
        r#"{"version":1,"actions":[{"type":"drag","from":{"x":10,"y":10},"to":{"x":50,"y":10},"steps":4},{"type":"key","keys":"ctrl+s"}]}"#,
    )
    .unwrap();
    let floor_s = floor.display().to_string();
    let script_s = script_path.display().to_string();
    let (code, stdout, stderr) = run(
        &["run", &script_s, "--dry-run"],
        &[("AI_CU_DEV_FLOOR", &floor_s)],
        b"",
    );
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["plan"].as_array().unwrap().len(), 2);
}

/// 一手ずつの操作 (D-052): 操作ごとのサブコマンドも、実際のバイナリで dry-run が装置に触れず、負の座標を受ける
#[test]
fn step_child_click_with_shot_dry_run_outputs_plan() {
    let dir = temp_dir("step");
    let floor = dir.join("floor.toml");
    std::fs::write(
        &floor,
        "version = 1\n[general]\ninput_enabled = true\ncapture_enabled = true\n",
    )
    .unwrap();
    let floor_s = floor.display().to_string();
    let click = |y: &str| {
        run(
            &[
                "click",
                "10",
                y,
                "--shot",
                "a.png",
                "--region",
                "0,0,10,10",
                "--dry-run",
            ],
            &[("AI_CU_DEV_FLOOR", &floor_s)],
            b"",
        )
    };
    let (code, stdout, stderr) = click("10");
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(v["dry_run"], true);
    assert_eq!(
        v["plan"].as_array().unwrap().len(),
        3,
        "click, wait, screenshot"
    );
    assert!(
        std::fs::metadata(dir.join("a.png")).is_err(),
        "dry-run では撮らない"
    );
    // 実際のバイナリの dry-run も画面の大きさ (読み取りだけ) を読む (D-058。main は DPI 認識を宣言している)
    let screen = &v["screen"];
    assert!(screen["w"].as_u64().is_some_and(|w| w > 0), "{v}");
    let (sx, sy) = (screen["x"].as_i64().unwrap(), screen["y"].as_i64().unwrap());
    let (sw, sh) = (screen["w"].as_i64().unwrap(), screen["h"].as_i64().unwrap());
    // 負の座標も引数として読める (使い方の誤り 2 にならない)。範囲の判定は画面の配置で決まる
    // (主モニタの上にモニタがあれば (10, -5) は画面の中で 0、無ければ 3)
    let inside = sx <= 10 && 10 < sx + sw && sy <= -5 && -5 < sy + sh;
    let (code, stdout, stderr) = click("-5");
    assert_eq!(
        code,
        if inside { 0 } else { 3 },
        "stdout={stdout} stderr={stderr}"
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(v["plan"][0]["steps"][0]["target"]["points"][0]["y"], -5);
    if !inside {
        assert!(stderr.contains("画面"), "{stderr}");
    }
}

/// 未知のキーの失敗の `issues[0].message` に使える名前が出る (D-131)
#[test]
fn key_dry_run_unknown_name_lists_usable_names() {
    let dir = temp_dir("key-unknown");
    let floor = dir.join("floor.toml");
    std::fs::write(&floor, "version = 1\n[general]\ninput_enabled = true\n").unwrap();
    let floor_s = floor.display().to_string();
    let (code, stdout, stderr) = run(
        &["key", "nope", "--dry-run"],
        &[("AI_CU_DEV_FLOOR", &floor_s)],
        b"",
    );
    assert_eq!(code, 3, "stdout={stdout} stderr={stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    let message = v["issues"][0]["message"].as_str().unwrap_or_default();
    assert!(message.contains("kanji"), "{v}");
}

/// close の対象を --window で渡した usage の誤りに、対象は位置引数だと伝える hint が
/// JSON と標準エラーの両方に載る。--window が無い誤りには載せない (D-086)
#[test]
fn close_with_window_flag_hints_positional_process() {
    let (code, stdout, stderr) = run(
        &["close", "--window", "notepad.exe", "--title", "x"],
        &[],
        b"",
    );
    assert_eq!(code, 2, "stdout={stdout} stderr={stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(v["error"]["code"], "usage", "{v}");
    let hint = v["error"]["hint"].as_str().unwrap_or_default();
    assert!(hint.contains("位置引数"), "{v}");
    assert!(stderr.contains("ヒント:"), "{stderr}");
    assert!(stderr.contains("位置引数"), "{stderr}");

    let (code, stdout, stderr) = run(&["close", "notepad.exe"], &[], b"");
    assert_eq!(code, 2, "stdout={stdout} stderr={stderr}");
    assert!(!stderr.contains("ヒント:"), "{stderr}");
}

#[test]
fn config_child_path_in_dev_shell_does_not_resolve_user_location() {
    let (code, stdout, _) = run(&["config", "path"], &[], b"");
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    assert_eq!(v["paths"]["dev_mode"], true);
    assert_eq!(v["paths"]["floor"]["kind"], "absent");
}

#[test]
fn config_child_broken_overlay_exits_5() {
    let dir = temp_dir("broken");
    let f = dir.join("bad.toml");
    std::fs::write(&f, "version = 1\n[allow]\nwindows = []\n").unwrap();
    let arg = format!("--config={}", f.display());
    let (code, stdout, _) = run(&["config", "validate", &arg], &[], b"");
    assert_eq!(code, 5, "{stdout}");
}

#[test]
fn validate_child_guide_is_markdown_for_ai_and_errors_carry_hints() {
    // guide は要点版 (手順の節は無く、全文の出し方の断り書きがある)。guide --full は全文 (D-097)
    let (code, summary, _) = run(&["guide"], &[], b"");
    assert_eq!(code, 0);
    let (code, full, _) = run(&["guide", "--full"], &[], b"");
    assert_eq!(code, 0);
    for section in [
        "# ai-cu ガイド",
        "## 座標",
        "## 終了コードと次の手",
        "## してはいけないこと",
    ] {
        assert!(summary.contains(section), "{section}");
        assert!(full.contains(section), "{section}");
    }
    assert!(!summary.contains("## 基本の流れ"), "{summary}");
    assert!(summary.contains("これは要点版。"), "{summary}");
    assert!(summary.contains("ai-cu guide --full"), "{summary}");
    assert!(full.contains("## 基本の流れ"), "{full}");
    assert!(!full.contains("これは要点版。"), "{full}");
    assert!(full.len() > summary.len());
    let (code, stdout, _) = run(&["validate", "-"], &[], b"{");
    assert_eq!(code, 3);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("JSON");
    assert!(
        v["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("ai-cu schema"),
        "{stdout}"
    );
    let (_, help, _) = run(&["--help"], &[], b"");
    assert!(help.contains("ai-cu guide"), "{help}");
}

#[test]
fn guide_setup_prints_own_exe_path() {
    let dir = temp_dir("setup");
    let floor = dir.join("config.toml");
    let floor_s = floor.display().to_string();
    let (code, stdout, stderr) = run(&["guide", "--setup"], &[("AI_CU_DEV_FLOOR", &floor_s)], b"");
    assert_eq!(code, 0, "{stderr}");
    let bin = std::path::Path::new(BIN);
    let name = bin.file_name().unwrap().to_string_lossy().into_owned();
    let parent = bin
        .parent()
        .and_then(|p| p.file_name())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    // canonicalize の差 (短い名前や \\?\) に備えて、ファイル名と親フォルダ名で照合する
    assert!(
        stdout.contains(&format!("{parent}\\{name}")),
        "{parent}\\{name}: {stdout}"
    );
    assert!(stdout.contains("## 3. MCP の登録"), "{stdout}");
    assert!(stdout.contains(&floor_s), "{stdout}");
    assert!(!floor.exists(), "guide --setup は設定ファイルを作らない");
}

#[test]
fn cli_child_help_mentions_exit_codes() {
    let (code, stdout, _) = run(&["--help"], &[], b"");
    assert_eq!(code, 0);
    assert!(stdout.contains("9 開発シェルでの実入力の拒否"), "{stdout}");
    assert!(stdout.contains("10 人が操作中"), "{stdout}");
    assert!(stdout.contains("11 前面にできない"), "{stdout}");
    assert!(stdout.contains("14 画面がロックかセーバー"), "{stdout}");
    assert!(stdout.contains("15 条件が時間内に"), "{stdout}");
}

/// `--version` は build.rs が埋めた git の印つきの版 (`ai-cu 0.1.0+g<12 桁>[.dirty]`) を出す (D-069)
#[test]
fn version_flag_prints_build_stamp() {
    let (code, stdout, stderr) = run(&["--version"], &[], b"");
    assert_eq!(code, 0, "stderr={stderr}");
    assert_eq!(
        stdout.trim(),
        format!("ai-cu {}", ai_cu::build_info::VERSION),
        "{stdout}"
    );
}

// ---------------------------------------------------------------------------
// MCP サーバーモード (D-055)。実バイナリを開発シェルの変数つきで起動し、標準入出力の JSON-RPC で確かめる。
// 実入力は最下層が 9 で断る (debug ビルドの mcp は実入力の道具を 9 で断る) ので、実デスクトップには触れない

const META: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

fn mcp_lines(stdout: &str) -> Vec<serde_json::Value> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| panic!("標準出力に JSON 以外: {l:?} ({e})"))
        })
        .collect()
}

#[test]
fn mcp_child_discover_list_guide_click_then_eof_exits_0_with_json_only_stdout() {
    let input = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"server/discover\",\"params\":{{{META}}}}}\n\
         {{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{{{META}}}}}\n\
         {{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{{\"name\":\"guide\",\"arguments\":{{}},{META}}}}}\n\
         {{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{{\"name\":\"click\",\"arguments\":{{\"x\":10,\"y\":10}},{META}}}}}\n\
         {{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{{\"name\":\"click\",\"arguments\":{{\"x\":10,\"y\":10,\"dry_run\":true}},{META}}}}}\n"
    );
    let (code, stdout, stderr) = run(&["mcp"], &[], input.as_bytes());
    assert_eq!(code, 0, "stderr={stderr}");
    let lines = mcp_lines(&stdout);
    assert_eq!(lines.len(), 5, "{stdout}");
    assert_eq!(lines[0]["id"], 1);
    assert_eq!(
        lines[0]["result"]["supportedVersions"],
        serde_json::json!(["2026-07-28"])
    );
    assert_eq!(lines[0]["result"]["resultType"], "complete");
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 19);
    // tools/list は設定 (承認の印) で変わるので private、server/discover は public のまま (D-078)
    assert_eq!(lines[1]["result"]["cacheScope"], "private");
    assert_eq!(lines[0]["result"]["cacheScope"], "public");
    assert!(
        lines[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("# ai-cu ガイド")
    );
    // debug ビルドの mcp は実入力を 9 で断る (release なら開発シェルの変数で 9、床なしで 4)
    assert_eq!(lines[3]["result"]["isError"], true, "{}", lines[3]);
    let v: serde_json::Value =
        serde_json::from_str(lines[3]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(
        matches!(v["error"]["exit_code"].as_i64(), Some(9) | Some(4)),
        "{v}"
    );
    // dry_run は床なしの 4 (装置に触れない)
    let v: serde_json::Value =
        serde_json::from_str(lines[4]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(v["error"]["exit_code"], 4, "{v}");
    assert_eq!(v["dry_run"], true);
    assert!(
        stderr.contains("host:"),
        "起動時にホストを記録する: {stderr}"
    );
    assert!(stderr.contains("標準入力が閉じた"), "{stderr}");
}

#[test]
fn mcp_child_legacy_initialize_initialized_tools_list_ping_then_eof() {
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#,
        "\n",
    );
    let (code, stdout, stderr) = run(&["mcp"], &[], input.as_bytes());
    assert_eq!(code, 0, "stderr={stderr}");
    let lines = mcp_lines(&stdout);
    assert_eq!(lines.len(), 3, "{stdout}");
    let by_id = |id: i64| {
        lines
            .iter()
            .find(|l| l["id"] == id)
            .unwrap_or_else(|| panic!("id {id}: {stdout}"))
    };
    assert_eq!(by_id(1)["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        by_id(1)["result"]["capabilities"],
        serde_json::json!({ "tools": {} })
    );
    assert_eq!(by_id(2)["result"]["tools"].as_array().unwrap().len(), 19);
    assert_eq!(by_id(3)["result"], serde_json::json!({}));
}

#[test]
fn mcp_child_exits_0_promptly_on_eof_without_requests() {
    let (out, elapsed) = wait_with_timeout(command(&["mcp"]).spawn().expect("spawn"), b"");
    assert_eq!(out.status.code(), Some(0));
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// `--log` のファイルに、起動、取り消しの受信、終了が時刻つきで追記される (D-061)
#[test]
fn mcp_child_log_file_records_start_cancel_and_eof() {
    let dir = temp_dir("mcp-log");
    let log = dir.join("mcp.log");
    let log_s = log.display().to_string();
    let input = b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":77}}\n";
    let (code, stdout, _) = run(&["mcp", "--log", &log_s], &[], input);
    assert_eq!(code, 0);
    assert!(stdout.is_empty(), "{stdout}");
    let text = std::fs::read_to_string(&log).expect("ログのファイル");
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains("を起動"), "{text}");
    // 起動ログの版は --version と serverInfo と同じ定数 (D-069)
    assert!(lines[0].contains(ai_cu::build_info::VERSION), "{text}");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("取り消しの通知 (requestId=77)")),
        "{text}"
    );
    assert!(lines.last().unwrap().contains("標準入力が閉じた"), "{text}");
    assert!(
        lines
            .iter()
            .all(|l| l.contains("Z (") && l.contains("ai-cu mcp: ")),
        "{text}"
    );
}

#[test]
fn mcp_child_shot_in_dev_shell_is_refused_and_writes_no_file() {
    let dir = temp_dir("mcp-shot");
    let floor = dir.join("floor.toml");
    // 無操作の判定 (10) は実デスクトップの最後の入力で決まり、人が操作中だと 9 より先に当たるので外す。
    // 入力デスクトップの判定 (14、D-066) は実画面の状態で 9 より先に当たりうるので、9 か 14 を受ける。意図は「撮らない」
    std::fs::write(
        &floor,
        "version = 1\n[general]\ncapture_enabled = true\n[capture]\nmin_idle_ms = 0\n",
    )
    .unwrap();
    let png = dir.join("never.png");
    let input = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{{\"name\":\"shot\",\"arguments\":{{\"region\":{{\"x\":0,\"y\":0,\"w\":8,\"h\":8}},\"path\":{}}},{META}}}}}\n",
        serde_json::json!(png.display().to_string())
    );
    let floor_s = floor.display().to_string();
    let (code, stdout, stderr) = run(&["mcp"], &[("AI_CU_DEV_FLOOR", &floor_s)], input.as_bytes());
    assert_eq!(code, 0, "stderr={stderr}");
    let lines = mcp_lines(&stdout);
    assert_eq!(lines.len(), 1, "{stdout}");
    let v: serde_json::Value =
        serde_json::from_str(lines[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let exit = v["error"]["exit_code"].as_i64();
    assert!(matches!(exit, Some(9) | Some(14)), "{v}");
    assert!(std::fs::metadata(&png).is_err(), "撮っていない");
}
