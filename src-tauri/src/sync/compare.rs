use sha2::{Digest, Sha256};

/// 行尾规范化：CRLF → LF（spec §6.5，比较基准统一）
pub fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// SHA-256 十六进制
pub fn sha256_hex(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// 三方比较输入。方向开关不在此处：它只决定 Upload/Download 是否执行，不豁免冲突（spec 约束）。
#[derive(Debug, Clone, PartialEq)]
pub struct CompareInput {
    pub local_exists: bool,
    pub local_hash: Option<String>,
    pub snapshot_hash: Option<String>,
    pub remote_file_exists: bool,
    pub remote_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 双方均与快照一致（或无可比内容）→ 不动作
    Noop,
    /// 首次映射且两边内容一致 → 以该内容播种快照，不写任何一侧
    SeedSnapshot,
    Upload,
    Download,
    /// 双方都偏离快照（或首次映射但内容不一致）→ 人工裁决
    Conflict,
    /// 本地文件不存在/被删除
    MissingLocal,
    /// 远端 gist 内目标文件不存在（此前同步过）
    MissingRemote,
}

/// 三方比较决策表（spec §6.1/6.2/6.5）：
/// 1. !local_exists → MissingLocal
/// 2. snapshot 存在 && !remote_file_exists → MissingRemote
/// 3. snapshot 不存在 && !remote_file_exists → Upload（往 gist 新增该文件）
/// 4. snapshot 不存在：local == remote → SeedSnapshot；不等 → Conflict
/// 5. snapshot 存在：local==snap && remote==snap → Noop；local≠snap && remote==snap → Upload；
///    local==snap && remote≠snap → Download；双方都 ≠ snap → Conflict
pub fn decide(input: &CompareInput) -> Decision {
    let local = if input.local_exists {
        input.local_hash.as_deref()
    } else {
        return Decision::MissingLocal;
    };
    let snapshot = input.snapshot_hash.as_deref();
    let remote = if input.remote_file_exists {
        input.remote_hash.as_deref()
    } else {
        return match snapshot {
            Some(_) => Decision::MissingRemote,
            None => Decision::Upload,
        };
    };

    match (snapshot, local, remote) {
        (None, l, r) => {
            if l == r {
                Decision::SeedSnapshot
            } else {
                Decision::Conflict
            }
        }
        (Some(snap), l, r) => match (l == Some(snap), r == Some(snap)) {
            (true, true) => Decision::Noop,
            (false, true) => Decision::Upload,
            (true, false) => Decision::Download,
            (false, false) => Decision::Conflict,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(local: Option<&str>, snap: Option<&str>, remote: Option<&str>) -> CompareInput {
        CompareInput {
            local_exists: local.is_some(),
            local_hash: local.map(|s| s.to_string()),
            snapshot_hash: snap.map(|s| s.to_string()),
            remote_file_exists: remote.is_some(),
            remote_hash: remote.map(|s| s.to_string()),
        }
    }

    #[test]
    fn normalize_converts_crlf_to_lf() {
        assert_eq!(normalize("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(normalize("a\nb"), "a\nb");
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn decision_table() {
        use Decision::*;
        let cases: Vec<(CompareInput, Decision)> = vec![
            // 1 本地缺失
            (input(None, Some("s"), Some("s")), MissingLocal),
            (input(None, None, None), MissingLocal),
            // 2 快照存在且远端文件没了
            (input(Some("s"), Some("s"), None), MissingRemote),
            // 3 首次映射且远端无该文件 → 上传新增
            (input(Some("l"), None, None), Upload),
            // 4 首次映射：一致 → 播种；不一致 → 冲突
            (input(Some("x"), None, Some("x")), SeedSnapshot),
            (input(Some("l"), None, Some("r")), Conflict),
            // 5 常规三方
            (input(Some("s"), Some("s"), Some("s")), Noop),
            (input(Some("l"), Some("s"), Some("s")), Upload),
            (input(Some("s"), Some("s"), Some("r")), Download),
            (input(Some("l"), Some("s"), Some("r")), Conflict),
        ];
        for (i, (inp, expected)) in cases.iter().enumerate() {
            assert_eq!(&decide(inp), expected, "case #{i}: {inp:?}");
        }
    }

    #[test]
    fn same_content_different_eol_hashes_equal_after_normalize() {
        let crlf = "line1\r\nline2\r\n";
        let lf = "line1\nline2\n";
        assert_ne!(crlf, lf);
        assert_eq!(sha256_hex(&normalize(crlf)), sha256_hex(&normalize(lf)));
    }
}
