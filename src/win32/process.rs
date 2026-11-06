//! 親プロセスの連鎖 (ホスト) の取得 (D-055)。ai-cu を起動した側 (エージェント、シェル、端末、Claude Desktop など) の
//! ウィンドウを、設定にかかわらず入力の届く先や `focus` / 撮影 / 要素の読み取りの対象にしないために使う。
//! 読み取りだけで、入力も状態の変更も起こさない。
//!
//! PID は再利用されるので、親の作成時刻が子より前であることを `GetProcessTimes` で確かめる。確かめられない
//! (開けないプロセス) とき、または作成時刻が逆転している (親が子より後 = 再利用された PID か、壁時計の
//! 巻き戻し) ときは、その親を含めて打ち切る (除外が増える側 = 安全側。作成時刻は壁時計で単調ではない)。
//! シェル (explorer.exe) は「起動した側」ではない (エクスプローラー、デスクトップ、タスクバーを巻き込む) ので、
//! その手前で止める (スタートメニューからの起動では親が sihost.exe のこともあり、その先はサービスで止まる)。
//! 旧来のコンソール (conhost.exe、OpenConsole.exe) はシェルの子で祖先には無いので、祖先を親に持つものを足す。
//! 届かない構成 (設定の deny が床): 既定の端末が Windows Terminal で COM のハンドオフを経た WT の
//! ウィンドウ、WSL からの起動 (端末が祖先に無い)。

use std::collections::HashMap;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// 連鎖をたどる深さの上限 (通常は数段)
const MAX_DEPTH: usize = 32;
/// ここで止める (含めない): シェル
const SHELL_EXES: [&str; 1] = ["explorer.exe"];
/// 祖先を親に持てばホストに足す: 旧来のコンソールのウィンドウを持つプロセス
const CONSOLE_EXES: [&str; 2] = ["conhost.exe", "openconsole.exe"];

/// ホスト側の 1 プロセス。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProcess {
    pub pid: u32,
    /// 実行ファイル名 (取れなければ空)
    pub exe: String,
}

/// 親の連鎖を純粋にたどる。`parents` は pid → (親の pid, 実行ファイル名)、`created` は pid → 作成時刻
/// (壁時計。逆転しうる。取れなければ None)。自分自身は含めない。
/// 打ち切る条件: 親が 0 か表に無い、循環、深さの上限、シェル (explorer.exe。含めない)、
/// 作成時刻が逆転 (親が子より後 = PID の再利用か時計の巻き戻し) か、どちらか不明 (その親は含めて、
/// それ以上はたどらない。無関係のプロセスを 1 つ余分に除外しうるが、真の親を落とさない)。
/// 最後に、祖先 (含めた親と自分) を親に持つコンソール (conhost.exe、OpenConsole.exe) を足す。
pub fn walk_ancestors(
    start: u32,
    parents: &HashMap<u32, (u32, String)>,
    created: &dyn Fn(u32) -> Option<u64>,
) -> Vec<HostProcess> {
    let mut chain = Vec::new();
    let mut seen = vec![start];
    let mut cur = start;
    let mut cur_created = created(start);
    while chain.len() < MAX_DEPTH {
        let Some((parent, _)) = parents.get(&cur) else {
            break;
        };
        let parent = *parent;
        if parent == 0 || seen.contains(&parent) {
            break;
        }
        let Some((_, exe)) = parents.get(&parent) else {
            break;
        };
        if SHELL_EXES.contains(&exe.to_ascii_lowercase().as_str()) {
            break;
        }
        let parent_created = created(parent);
        chain.push(HostProcess {
            pid: parent,
            exe: exe.clone(),
        });
        seen.push(parent);
        match (cur_created, parent_created) {
            (Some(c), Some(p)) if p <= c => {}
            _ => break,
        }
        cur = parent;
        cur_created = parent_created;
    }
    let consoles: Vec<HostProcess> = parents
        .iter()
        .filter(|(pid, (ppid, exe))| {
            **pid != start
                && (*ppid == start || chain.iter().any(|h| h.pid == *ppid))
                && CONSOLE_EXES.contains(&exe.to_ascii_lowercase().as_str())
                && !chain.iter().any(|h| h.pid == **pid)
        })
        .map(|(pid, (_, exe))| HostProcess {
            pid: *pid,
            exe: exe.clone(),
        })
        .collect();
    let mut consoles = consoles;
    consoles.sort_by_key(|h| h.pid);
    chain.extend(consoles);
    chain
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: CreateToolhelp32Snapshot / OpenProcess が返した有効なハンドルで、ここで 1 回だけ閉じる。
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// いまのプロセスの一覧 (pid → (親の pid, 実行ファイル名))。取れなければ誤り (GetLastError つき)。
fn snapshot() -> Result<HashMap<u32, (u32, String)>, String> {
    let mut table = HashMap::new();
    // SAFETY: フラグと 0 (全プロセス) を渡すだけ。失敗すれば INVALID_HANDLE_VALUE が返る。
    let h = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if h.is_null() || h == INVALID_HANDLE_VALUE {
        return Err(format!(
            "CreateToolhelp32Snapshot が失敗 (GetLastError={})",
            super::last_error()
        ));
    }
    let snap = OwnedHandle(h);
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: snap は有効なスナップショットのハンドル。entry は dwSize を設定した (API の契約) 書き込み可能な構造体。
    let mut ok = unsafe { Process32FirstW(snap.0, &mut entry) };
    if ok == 0 {
        return Err(format!(
            "Process32FirstW が失敗 (GetLastError={})",
            super::last_error()
        ));
    }
    while ok != 0 {
        let name_len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let exe = String::from_utf16_lossy(&entry.szExeFile[..name_len]);
        table.insert(entry.th32ProcessID, (entry.th32ParentProcessID, exe));
        // SAFETY: 同上。Process32NextW は entry に次の要素を書き、無ければ 0 を返す。
        ok = unsafe { Process32NextW(snap.0, &mut entry) };
    }
    Ok(table)
}

fn filetime_u64(t: &FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// プロセスの作成時刻 (FILETIME を 64 ビットにしたもの)。開けなければ None。
fn creation_time(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    // SAFETY: 権限の定数と pid を渡すだけ。失敗すれば null が返る。
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let h = OwnedHandle(h);
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: h は PROCESS_QUERY_LIMITED_INFORMATION で開いた有効なハンドル。4 つの出力はすべて書き込み可能な FILETIME
    // (省略できる引数は無い)。
    let ok = unsafe { GetProcessTimes(h.0, &mut created, &mut exited, &mut kernel, &mut user) };
    if ok == 0 {
        None
    } else {
        Some(filetime_u64(&created))
    }
}

/// このプロセスを起動した側の連鎖 (直接の親から順に) とコンソール。プロセスの一覧が取れなければ誤り
/// (呼び出し側が記録し、MCP では実入力を断る)。
pub fn host_chain() -> Result<Vec<HostProcess>, String> {
    // SAFETY: 引数なし。自分の pid を返すだけ。
    let me = unsafe { GetCurrentProcessId() };
    Ok(walk_ancestors(me, &snapshot()?, &creation_time))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[(u32, u32, &str)]) -> HashMap<u32, (u32, String)> {
        rows.iter()
            .map(|(pid, ppid, exe)| (*pid, (*ppid, exe.to_string())))
            .collect()
    }

    fn times(rows: &[(u32, u64)]) -> impl Fn(u32) -> Option<u64> {
        let m: HashMap<u32, u64> = rows.iter().copied().collect();
        move |pid| m.get(&pid).copied()
    }

    fn pids(chain: &[HostProcess]) -> Vec<u32> {
        chain.iter().map(|h| h.pid).collect()
    }

    #[test]
    fn host_chain_walks_parents_in_order_and_stops_at_pid_zero() {
        let t = table(&[
            (100, 50, "ai-cu.exe"),
            (50, 10, "node.exe"),
            (10, 4, "WindowsTerminal.exe"),
            (4, 0, "System"),
        ]);
        let c = times(&[(100, 400), (50, 300), (10, 200), (4, 100)]);
        let chain = walk_ancestors(100, &t, &c);
        assert_eq!(pids(&chain), vec![50, 10, 4]);
        assert_eq!(chain[0].exe, "node.exe");
        assert_eq!(chain[2].exe, "System");
        // 自分自身は含めない。表に無い親で打ち切る
        let t2 = table(&[(100, 50, "ai-cu.exe"), (50, 999, "node.exe")]);
        assert_eq!(pids(&walk_ancestors(100, &t2, &c)), vec![50]);
        assert!(walk_ancestors(7, &t2, &c).is_empty(), "表に無い開始点");
    }

    #[test]
    fn host_chain_stops_on_cycles_reused_pids_and_depth_limit() {
        // 循環: 100 → 50 → 100
        let t = table(&[(100, 50, "a"), (50, 100, "b")]);
        let c = times(&[(100, 400), (50, 300)]);
        assert_eq!(pids(&walk_ancestors(100, &t, &c)), vec![50]);
        // 自分を親に持つ (起こらないはずだが打ち切る)
        let t = table(&[(100, 100, "a")]);
        assert!(walk_ancestors(100, &t, &c).is_empty());
        // 作成時刻の逆転: 親 (50) が子 (100) より後 → PID の再利用か時計の巻き戻し。その親は含めて (安全側)、
        // それ以上はたどらない (win32 レビューの中-1: 壁時計は単調ではないので、真の親を落とさない)
        let t = table(&[(100, 50, "a"), (50, 10, "b"), (10, 0, "c")]);
        let c = times(&[(100, 400), (50, 500), (10, 100)]);
        assert_eq!(pids(&walk_ancestors(100, &t, &c)), vec![50]);
        // 同時刻 (同じ tick) は親として認める
        let c = times(&[(100, 400), (50, 400), (10, 100)]);
        assert_eq!(pids(&walk_ancestors(100, &t, &c)), vec![50, 10]);
        // 深さの上限
        let rows: Vec<(u32, u32, &str)> = (1..=100).map(|p| (p, p + 1, "x")).collect();
        let t = table(&rows);
        let all: Vec<(u32, u64)> = (1..=101).map(|p| (p, 1000 - u64::from(p))).collect();
        let c = times(&all);
        assert_eq!(walk_ancestors(1, &t, &c).len(), MAX_DEPTH);
    }

    #[test]
    fn host_chain_includes_unverifiable_parent_but_stops_there() {
        // 親の作成時刻が取れない (開けない) → 安全側に含めて、それ以上はたどらない
        let t = table(&[(100, 50, "a"), (50, 10, "b"), (10, 0, "c")]);
        let c = times(&[(100, 400), (10, 100)]);
        assert_eq!(pids(&walk_ancestors(100, &t, &c)), vec![50]);
        // 自分の作成時刻が取れない → 直接の親だけ含める
        let c = times(&[(50, 300), (10, 100)]);
        assert_eq!(pids(&walk_ancestors(100, &t, &c)), vec![50]);
        // 含めて打ち切った親 (50) を親に持つコンソールも足す (祖先 = 含めた親 + 自分)
        let t = table(&[
            (100, 50, "a"),
            (50, 10, "b"),
            (10, 0, "c"),
            (51, 50, "conhost.exe"),
            (11, 10, "conhost.exe"),
        ]);
        let c = times(&[(100, 400), (10, 100)]);
        assert_eq!(pids(&walk_ancestors(100, &t, &c)), vec![50, 51]);
    }

    #[test]
    fn host_chain_stops_before_explorer_and_adds_consoles_of_ancestors() {
        // Claude Desktop をスタートメニューから起動: claude.exe ← explorer.exe ← ...。explorer は含めない
        let t = table(&[
            (100, 50, "ai-cu.exe"),
            (50, 20, "claude.exe"),
            (20, 10, "Explorer.EXE"),
            (10, 0, "winlogon.exe"),
            // 旧来のコンソール: シェル (50 の子ではなく 60) の子として conhost がいる
            (60, 20, "cmd.exe"),
            (61, 60, "conhost.exe"),
            // 祖先 (50) を親に持つ OpenConsole はホストに足す。自分 (100) を親に持つものも足す
            (70, 50, "OpenConsole.exe"),
            (71, 100, "conhost.exe"),
            // 祖先の子でも、コンソールでなければ足さない
            (80, 50, "code.exe"),
        ]);
        let c = times(&[
            (100, 500),
            (50, 400),
            (20, 300),
            (10, 100),
            (70, 450),
            (71, 510),
        ]);
        let chain = walk_ancestors(100, &t, &c);
        assert_eq!(pids(&chain), vec![50, 70, 71]);
        assert_eq!(chain[1].exe, "OpenConsole.exe");
    }

    #[test]
    fn host_chain_real_contains_a_parent_or_is_empty() {
        // 読み取りだけ。テストの実行ファイルは cargo (または rustc のテストランナー) から起動されるので、通常は 1 つ以上
        let chain = host_chain().expect("プロセスの一覧が取れる");
        for h in &chain {
            assert_ne!(h.pid, std::process::id());
        }
        println!("host chain: {chain:?}");
    }
}
