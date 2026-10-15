//! 真实 GitHub Gist API 集成测试。
//! 默认跳过（#[ignore]）；设置环境变量 GISTSYNC_E2E_TOKEN（classic PAT，gist scope）后：
//!   GISTSYNC_E2E_TOKEN=ghp_xxx cargo test e2e -- --ignored
//! 用专用测试账号的 token，测试会创建/删除自己的 gist fixture。

use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;

use gist_auto_sync_lib::config::ConfigStore;
use gist_auto_sync_lib::model::{Account, Mapping, ProviderKind};
use gist_auto_sync_lib::provider::{GistProvider, ProviderError};
use gist_auto_sync_lib::provider::github::GithubProvider;
use gist_auto_sync_lib::sync::compare::{normalize, sha256_hex};
use gist_auto_sync_lib::sync::engine::{run_sync_cycle, MappingStatus, ProviderCache, StatusEvent, StatusSink, SyncCtx, Trigger};
use gist_auto_sync_lib::sync::history::HistoryReason;
use gist_auto_sync_lib::sync::service::Engine;
use gist_auto_sync_lib::sync::snapshot::{SelfWriteGuard, SnapshotStore, SyncDirection};

#[derive(Default)]
struct VecSink(std::sync::Mutex<Vec<StatusEvent>>);

impl StatusSink for VecSink {
    fn emit(&self, event: StatusEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn sha(s: &str) -> String {
    sha256_hex(&normalize(s))
}

async fn setup() -> (SyncCtx, Mapping, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let local_path = dir.path().join("gistsync-e2e.md");
    std::fs::write(&local_path, "e2e base content\n").unwrap();

    let config = ConfigStore::load_from(dir.path().join("config.json")).unwrap();
    config
        .update(|c| {
            c.accounts.push(Account {
                id: "e2e-acc".into(),
                name: "e2e".into(),
                provider: ProviderKind::Github,
                base_url: "https://api.github.com".into(),
                token_ref: "unused".into(),
            });
        })
        .unwrap();

    let snapshots = Arc::new(SnapshotStore::new(dir.path().join("snapshots")));
    let providers = ProviderCache::new();
    providers.insert(
        "e2e-acc",
        Arc::new(GithubProvider::new(
            std::env::var("GISTSYNC_E2E_TOKEN").expect("GISTSYNC_E2E_TOKEN 未设置"),
        )),
    );
    let ctx = SyncCtx {
        config: Arc::new(config),
        snapshots,
        providers,
        guard: SelfWriteGuard::new(),
        sink: Arc::new(VecSink::default()),
        synclog: Arc::new(gist_auto_sync_lib::synclog::SyncLogStore::load_from(
            dir.path().join("synclog.jsonl"),
        )),
    };

    let mapping = Mapping {
        id: "e2e-m".into(),
        name: String::new(),
        local_path: local_path.to_string_lossy().to_string(),
        account_id: "e2e-acc".into(),
        gist_id: String::new(), // 由用例填充
        gist_filename: "gistsync-e2e.md".into(),
        upload_enabled: true,
        download_enabled: true,
        enabled: true,
    };
    (ctx, mapping, dir)
}

async fn create_fixture_gist() -> String {
    // GitHub 创建 gist：POST /gists —— provider 未暴露 create，这里直接走 HTTP
    let token = std::env::var("GISTSYNC_E2E_TOKEN").unwrap();
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.github.com/gists")
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "gist-auto-sync-e2e")
        .json(&serde_json::json!({
            "description": "gist-auto-sync e2e fixture (auto-deleted)",
            "public": false,
            "files": { "gistsync-e2e.md": { "content": "e2e base content\n" } }
        }))
        .send()
        .await
        .expect("创建 gist 请求失败");
    assert!(resp.status().is_success(), "创建 gist 失败: {}", resp.status());
    let json: serde_json::Value = resp.json().await.unwrap();
    json["id"].as_str().expect("响应缺少 id").to_string()
}

async fn delete_fixture_gist(gist_id: &str) {
    let token = std::env::var("GISTSYNC_E2E_TOKEN").unwrap();
    let client = reqwest::Client::new();
    let _ = client
        .delete(format!("https://api.github.com/gists/{gist_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "gist-auto-sync-e2e")
        .send()
        .await;
}

/// RAII：测试结束（含 assert 中途失败/unwind）时 best-effort 删除远端 gist fixture。
/// Drop 可能发生在测试 runtime 线程上，不能嵌套 block_on——在独立线程建临时 runtime 执行。
struct GistGuard(String);

impl Drop for GistGuard {
    fn drop(&mut self) {
        let gist_id = self.0.clone();
        let _ = std::thread::spawn(move || {
            if std::env::var("GISTSYNC_E2E_TOKEN").is_err() {
                return;
            }
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(delete_fixture_gist(&gist_id));
        })
        .join();
    }
}

async fn remote_content(gist_id: &str) -> String {
    let token = std::env::var("GISTSYNC_E2E_TOKEN").unwrap();
    let provider = GithubProvider::new(token);
    let detail = provider
        .get_gist(gist_id, Some("gistsync-e2e.md"))
        .await
        .expect("读取 gist 失败");
    detail
        .files
        .into_iter()
        .find(|f| f.filename == "gistsync-e2e.md")
        .and_then(|f| f.content)
        .unwrap_or_default()
}

async fn patch_remote(gist_id: &str, content: &str) {
    let token = std::env::var("GISTSYNC_E2E_TOKEN").unwrap();
    let client = reqwest::Client::new();
    let resp = client
        .patch(format!("https://api.github.com/gists/{gist_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "gist-auto-sync-e2e")
        .json(&serde_json::json!({
            "files": { "gistsync-e2e.md": { "content": content } }
        }))
        .send()
        .await
        .expect("PATCH gist 请求失败");
    assert!(resp.status().is_success(), "PATCH gist 失败");
}

/// GitHub 限流（网络重试）时给点余量
async fn patient_cycle(ctx: &SyncCtx, mapping: &Mapping, trigger: Trigger) -> MappingStatus {
    for attempt in 1..=3 {
        match run_sync_cycle(ctx, mapping, trigger).await {
            MappingStatus::Error => {
                if attempt == 3 {
                    return MappingStatus::Error;
                }
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
            }
            other => return other,
        }
    }
    MappingStatus::Error
}

#[tokio::test]
#[ignore = "需要真实 GitHub token：设置 GISTSYNC_E2E_TOKEN 后 --ignored 运行"]
async fn e2e_github_roundtrip() {
    let (ctx, mut mapping, _dir) = setup().await;
    let gist_id = create_fixture_gist().await;
    let _fixture = GistGuard(gist_id.clone());
    mapping.gist_id = gist_id.clone();

    // 1. 首次对账：内容一致 → 建立基准
    let status = patient_cycle(&ctx, &mapping, Trigger::Poll).await;
    assert_eq!(status, MappingStatus::Idle, "首次对账应 Idle");
    assert_eq!(
        ctx.snapshots.load(&mapping.id).unwrap().last_synced_hash,
        sha("e2e base content\n")
    );

    // 2. 本地改 → 手动上传 → 远端内容更新
    std::fs::write(&mapping.local_path, "local v2\n").unwrap();
    let status = patient_cycle(&ctx, &mapping, Trigger::ManualUpload).await;
    assert_eq!(status, MappingStatus::Idle);
    assert_eq!(remote_content(&gist_id).await, "local v2\n");

    // 3. 远端改 → Poll → 本地更新（字节原样）
    patch_remote(&gist_id, "remote v3\n").await;
    let status = patient_cycle(&ctx, &mapping, Trigger::Poll).await;
    assert_eq!(status, MappingStatus::Idle);
    assert_eq!(
        std::fs::read_to_string(&mapping.local_path).unwrap(),
        "remote v3\n"
    );

    // 4. 双改 → Conflict
    std::fs::write(&mapping.local_path, "local v4\n").unwrap();
    patch_remote(&gist_id, "remote v5\n").await;
    let status = patient_cycle(&ctx, &mapping, Trigger::Poll).await;
    assert_eq!(status, MappingStatus::Conflict);

    // 5. 手动覆盖（用远端）→ 恢复 Idle，本地=远端
    let status = patient_cycle(&ctx, &mapping, Trigger::ManualDownload).await;
    assert_eq!(status, MappingStatus::Idle);
    assert_eq!(
        std::fs::read_to_string(&mapping.local_path).unwrap(),
        "remote v5\n"
    );
    let snapshot_state = ctx.snapshots.load(&mapping.id).unwrap();
    assert_eq!(snapshot_state.last_direction, Some(SyncDirection::Download));

    // 快照时间是最近时间
    assert!(snapshot_state.last_synced_at <= Utc::now());

    // fixture 清理由 _fixture guard 的 Drop 兜底（正常/失败路径均执行）
}

#[tokio::test]
#[ignore = "需要真实 GitHub token"]
async fn e2e_github_auth_error() {
    let token = std::env::var("GISTSYNC_E2E_TOKEN").unwrap();
    let provider = GithubProvider::new(format!("{token}invalid"));
    let err = provider.verify_token().await.unwrap_err();
    assert!(matches!(err, ProviderError::Auth), "got: {err:?}");
}

/// V0.3 回滚 + commits 历史 roundtrip：上传 v1/v2 → commits ≥2 → 回滚到 v1 归档
/// → 本地/远端拉齐为 v1，commits 增至 +1。
#[tokio::test]
#[ignore = "需要真实 GitHub token：设置 GISTSYNC_E2E_TOKEN 后 --ignored 运行"]
async fn rollback_commits_roundtrip() {
    let (ctx, mut mapping, dir) = setup().await;
    let ctx = Arc::new(ctx);
    let gist_id = create_fixture_gist().await;
    let _fixture = GistGuard(gist_id.clone());
    mapping.gist_id = gist_id.clone();
    let engine = Arc::new(Engine::new(ctx.clone(), dir.path().join("statuses.json")).unwrap());

    // 1. 首次对账建立基准；本地改 v1 → 上传（新 commit）
    let status = patient_cycle(&ctx, &mapping, Trigger::Poll).await;
    assert_eq!(status, MappingStatus::Idle, "首次对账应 Idle");
    std::fs::write(&mapping.local_path, "e2e v1\n").unwrap();
    let status = patient_cycle(&ctx, &mapping, Trigger::ManualUpload).await;
    assert_eq!(status, MappingStatus::Idle);
    assert_eq!(remote_content(&gist_id).await, "e2e v1\n");

    // 2. 本地改 v2 → 上传（覆盖前远端 v1 自动归档，reason=upload）
    std::fs::write(&mapping.local_path, "e2e v2\n").unwrap();
    let status = patient_cycle(&ctx, &mapping, Trigger::ManualUpload).await;
    assert_eq!(status, MappingStatus::Idle);
    assert_eq!(remote_content(&gist_id).await, "e2e v2\n");

    // 3. commits 历史 ≥ 2
    let token = std::env::var("GISTSYNC_E2E_TOKEN").unwrap();
    let provider = GithubProvider::new(token);
    let commits_before = provider.list_commits(&gist_id).await.expect("list_commits 失败");
    assert!(commits_before.len() >= 2, "commits={}", commits_before.len());

    // 4. 定位 v1 归档（上传 v2 时被归档的远端旧内容）
    let v1_entry = ctx
        .snapshots
        .history_list(&mapping.id)
        .into_iter()
        .find(|e| {
            e.reason == HistoryReason::Upload
                && ctx
                    .snapshots
                    .read_archive(&mapping.id, &e.file)
                    .is_ok_and(|c| c == "e2e v1\n")
        })
        .expect("应存在内容为 v1 的上传归档");

    // 5. 回滚到 v1：本地与远端同时恢复（网络瞬断重试，同 patient_cycle 模式）
    let mut status = engine.rollback_to_archive(&mapping.id, &v1_entry.file).await;
    for attempt in 1..=3 {
        if status != MappingStatus::Error {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
        status = engine.rollback_to_archive(&mapping.id, &v1_entry.file).await;
    }
    assert_eq!(status, MappingStatus::Idle, "回滚应成功");
    assert_eq!(
        std::fs::read_to_string(&mapping.local_path).unwrap(),
        "e2e v1\n",
        "本地应恢复为 v1"
    );
    assert_eq!(remote_content(&gist_id).await, "e2e v1\n", "远端应恢复为 v1");

    // 6. 回滚在远端产生一次新提交
    let commits_after = provider.list_commits(&gist_id).await.expect("list_commits 失败");
    assert_eq!(
        commits_after.len(),
        commits_before.len() + 1,
        "回滚应恰好新增一条 commit"
    );
}
