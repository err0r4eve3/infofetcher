//! Local originals and bounded, offline extraction. Imported bytes are never rendered as HTML.
use super::*;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Query},
    http::StatusCode,
};
use std::{path::Path as FilePath, process::Stdio};
use tokio::io::AsyncReadExt;

pub(super) const MAX_FILE: usize = 10 * 1024 * 1024;
const MAX_PAGES: usize = 20;
const MAX_PAGE_CHARS: usize = 30_000;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v2/osint/cases/{id}/documents",
            get(list).post(upload_case),
        )
        .route("/api/v2/events/{id}/osint/documents", post(upload_event))
        .route("/api/v2/osint/documents/{id}", get(detail))
        .route("/api/v2/osint/documents/{id}/original", get(original))
        .layer(DefaultBodyLimit::max(MAX_FILE))
        .layer(axum::middleware::map_response(
            |mut r: Response| async move {
                r.headers_mut()
                    .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
                r
            },
        ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadQuery {
    filename: String,
}

async fn upload_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> AppResult<impl IntoResponse> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    let mut tx = state.pool.begin().await?;
    let title: String =
        sqlx::query_scalar("SELECT representative_title FROM story_clusters WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AppError::NotFound)?;
    let case_id = casebook::ensure_case(&mut tx, id, &title).await?;
    let value = store(&mut tx, case_id, &actor, q, body).await?;
    tx.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(value)))
}
async fn upload_case(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> AppResult<impl IntoResponse> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    let mut tx = state.pool.begin().await?;
    let value = store(&mut tx, id, &actor, q, body).await?;
    tx.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(value)))
}
async fn store(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    case_id: Uuid,
    actor: &str,
    q: UploadQuery,
    body: Bytes,
) -> AppResult<Value> {
    let filename = q.filename.trim();
    if filename.is_empty()
        || filename.chars().count() > 180
        || filename
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
    {
        return Err(AppError::BadRequest(
            "文件名须为 1–180 字符且不能包含路径".into(),
        ));
    }
    let media = detect(&body)?;
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM osint_cases WHERE id = $1 FOR UPDATE")
        .bind(case_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AppError::NotFound)?;
    let hash = hex::encode(Sha256::digest(&body));
    if let Some(row) = sqlx::query("SELECT d.id, j.id AS job_id FROM osint_documents d JOIN osint_jobs j ON j.document_id = d.id WHERE d.case_id = $1 AND d.sha256 = $2")
        .bind(case_id).bind(&hash).fetch_optional(&mut **tx).await? {
        return Ok(json!({"case_id":case_id,"id":row.get::<Uuid,_>("id"),"job_id":row.get::<Uuid,_>("job_id"),"duplicate":true}));
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM osint_documents WHERE case_id = $1")
        .bind(case_id)
        .fetch_one(&mut **tx)
        .await?;
    if count >= 20 {
        return Err(AppError::BadRequest("每份档案最多保存 20 个文件".into()));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO osint_documents (id,case_id,filename,media_type,body,sha256,created_by) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(id).bind(case_id).bind(filename).bind(media).bind(body.as_ref()).bind(hash).bind(actor).execute(&mut **tx).await?;
    let job_id: Uuid = sqlx::query_scalar("INSERT INTO osint_jobs (case_id,document_id,kind,input,created_by) VALUES ($1,$2,'document','{}',$3) RETURNING id")
        .bind(case_id).bind(id).bind(actor).fetch_one(&mut **tx).await?;
    sqlx::query("UPDATE osint_cases SET updated_at = now() WHERE id = $1")
        .bind(case_id)
        .execute(&mut **tx)
        .await?;
    Ok(json!({"case_id":case_id,"id":id,"job_id":job_id,"duplicate":false}))
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    let items: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id', d.id, 'filename',filename,'media_type',media_type,'sha256',sha256,'created_at',d.created_at,'created_by',d.created_by,'status',d.status,'byte_length',octet_length(body),'extraction_sha256',extraction_sha256,'job_id',j.id,'error',j.error) FROM osint_documents d JOIN osint_jobs j ON j.document_id = d.id WHERE d.case_id = $1 ORDER BY d.created_at DESC,d.id DESC")
        .bind(id).fetch_all(&state.pool).await?;
    Ok(Json(json!({"items":items})))
}
async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    let value: Value = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'case_id',case_id,'filename',filename,'sha256',sha256,'status',status,'extraction',extraction,'extraction_sha256',extraction_sha256) FROM osint_documents WHERE id = $1")
        .bind(id).fetch_optional(&state.pool).await?.ok_or(AppError::NotFound)?;
    Ok(Json(value))
}
async fn original(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    auth::require_session(&state.pool, &headers).await?;
    let row = sqlx::query("SELECT body,filename FROM osint_documents WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    let filename: String = row.get("filename");
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"evidence-{id}\"; filename*=UTF-8''{}",
                    urlencoding::encode(&filename)
                ),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'".to_string(),
            ),
        ],
        row.get::<Vec<u8>, _>("body"),
    )
        .into_response())
}

fn detect(body: &[u8]) -> AppResult<&'static str> {
    if body.is_empty() || body.len() > MAX_FILE {
        return Err(AppError::BadRequest("文件须为 1 byte–10 MiB".into()));
    }
    if body.starts_with(b"%PDF-") {
        return Ok("application/pdf");
    }
    if body.starts_with(b"\x89PNG\r\n\x1a\n") {
        if body.len() < 24 || &body[12..16] != b"IHDR" {
            return Err(AppError::BadRequest("PNG 文件头损坏".into()));
        }
        dimensions(
            u32::from_be_bytes(body[16..20].try_into().unwrap()),
            u32::from_be_bytes(body[20..24].try_into().unwrap()),
        )?;
        return Ok("image/png");
    }
    if body.starts_with(b"\xff\xd8\xff") {
        let mut i = 2;
        while i + 4 <= body.len() {
            if body[i] != 0xff {
                break;
            }
            let tag = body[i + 1];
            i += 2;
            if tag == 0xff {
                i -= 1;
                continue;
            }
            if tag == 0xda || tag == 0xd9 {
                break;
            }
            if tag == 0x01 || (0xd0..=0xd7).contains(&tag) {
                continue;
            }
            let len = u16::from_be_bytes([body[i], body[i + 1]]) as usize;
            if len < 2 || i + len > body.len() {
                break;
            }
            if [
                0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7, 0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf,
            ]
            .contains(&tag)
                && len >= 7
            {
                dimensions(
                    u16::from_be_bytes([body[i + 5], body[i + 6]]) as u32,
                    u16::from_be_bytes([body[i + 3], body[i + 4]]) as u32,
                )?;
                return Ok("image/jpeg");
            }
            i += len;
        }
        return Err(AppError::BadRequest("JPEG 缺少有效尺寸信息".into()));
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| AppError::BadRequest("仅支持 PDF、PNG、JPEG 和 UTF-8 文本".into()))?;
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(AppError::BadRequest("文本包含二进制控制字符".into()));
    }
    Ok("text/plain")
}
fn dimensions(w: u32, h: u32) -> AppResult<()> {
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 20_000_000 {
        return Err(AppError::BadRequest(
            "图片尺寸必须在 2,000 万像素以内".into(),
        ));
    }
    Ok(())
}

pub(super) async fn extract(media: &str, body: &[u8]) -> anyhow::Result<Value> {
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("input");
    tokio::fs::write(&input, body).await?;
    let input_path = input.to_str().unwrap();
    let mut pages = vec![];
    if media == "text/plain" {
        pages.push(page(1, "utf8", std::str::from_utf8(body)?));
    } else if media == "application/pdf" {
        let info = command("pdfinfo", &[input_path], dir.path()).await?;
        let count = std::str::from_utf8(&info)?
            .lines()
            .find_map(|line| {
                line.strip_prefix("Pages:")
                    .and_then(|x| x.trim().parse::<usize>().ok())
            })
            .ok_or_else(|| anyhow::anyhow!("无法读取 PDF 页数"))?;
        anyhow::ensure!(
            (1..=MAX_PAGES).contains(&count),
            "PDF 须为 1–20 页；原件已保留，可拆分后重新导入"
        );
        for n in 1..=count {
            let number = n.to_string();
            let bytes = command(
                "pdftotext",
                &[
                    "-f", &number, "-l", &number, "-enc", "UTF-8", "-nopgbrk", input_path, "-",
                ],
                dir.path(),
            )
            .await?;
            let text = String::from_utf8(bytes)?;
            if text.chars().filter(|c| !c.is_whitespace()).count() >= 30 {
                pages.push(page(n, "pdf_text", &text));
            } else {
                let raster = dir.path().join("page");
                command(
                    "pdftoppm",
                    &[
                        "-f",
                        &number,
                        "-l",
                        &number,
                        "-singlefile",
                        "-scale-to",
                        "2000",
                        "-png",
                        input_path,
                        raster.to_str().unwrap(),
                    ],
                    dir.path(),
                )
                .await?;
                let image = dir.path().join("page.png");
                let text = ocr(&image, dir.path()).await?;
                pages.push(page(n, "ocr", &text));
                tokio::fs::remove_file(image).await?;
            }
        }
    } else {
        pages.push(page(1, "ocr", &ocr(&input, dir.path()).await?));
    }
    anyhow::ensure!(
        pages
            .iter()
            .any(|p| p["text"].as_str().is_some_and(|t| !t.trim().is_empty())),
        "未提取到文字；原件已保留"
    );
    Ok(
        json!({"format":"infofetcher-document-text-v1","pages":pages,"ocr_languages":"chi_sim+eng","raster_long_side":2000,"warning":"OCR 可能误识别，引用前请对照原件；导入时间不是事件时间。"}),
    )
}
fn page(number: usize, method: &str, text: &str) -> Value {
    let normalized = text.replace(['\r', '\0'], "");
    let value: String = normalized.chars().take(MAX_PAGE_CHARS).collect();
    json!({"page":number,"method":method,"text":value,"sha256":hex::encode(Sha256::digest(value.as_bytes())),"truncated":normalized.chars().count()>MAX_PAGE_CHARS})
}
async fn ocr(input: &FilePath, dir: &FilePath) -> anyhow::Result<String> {
    let output = command(
        "tesseract",
        &[input.to_str().unwrap(), "stdout", "-l", "chi_sim+eng"],
        dir,
    )
    .await?;
    Ok(String::from_utf8(output)?)
}
async fn command(program: &str, args: &[&str], dir: &FilePath) -> anyhow::Result<Vec<u8>> {
    // The executable and argument grammar are fixed here; callers cannot supply commands.
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LANG", "C")
        .env("OMP_THREAD_LIMIT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            for (resource, limit) in [
                (libc::RLIMIT_CPU, 30),
                (libc::RLIMIT_FSIZE, 32 * 1024 * 1024),
                (libc::RLIMIT_NOFILE, 64),
            ] {
                let value = libc::rlimit {
                    rlim_cur: limit,
                    rlim_max: limit,
                };
                if libc::setrlimit(resource, &value) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            #[cfg(target_os = "linux")]
            {
                let value = libc::rlimit {
                    rlim_cur: 1024 * 1024 * 1024,
                    rlim_max: 1024 * 1024 * 1024,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &value) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|_| anyhow::anyhow!("本地提取工具 {program} 无法启动；请检查工具及语言包安装"))?;
    let stdout = child.stdout.take().unwrap();
    tokio::time::timeout(Duration::from_secs(40), async {
        let mut bytes = vec![];
        stdout
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        anyhow::ensure!(bytes.len() <= 2 * 1024 * 1024, "{program} 输出超出限制");
        anyhow::ensure!(
            child.wait().await?.success(),
            "{program} 提取失败；文件可能损坏、加密或缺少 OCR 语言包"
        );
        Ok(bytes)
    })
    .await
    .map_err(|_| anyhow::anyhow!("{program} 提取超时"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_detection_is_byte_based_and_bounded() {
        assert_eq!(detect(b"%PDF-1.7\n").unwrap(), "application/pdf");
        assert_eq!(detect("公开记录".as_bytes()).unwrap(), "text/plain");
        assert!(detect(b"hello\0world").is_err());
        assert!(detect(&[]).is_err());
        assert!(dimensions(u32::MAX, u32::MAX).is_err());
        assert!(detect(b"\xff\xd8\xff\xda\0\0").is_err());
    }
    #[tokio::test]
    #[ignore = "requires local Poppler and Tesseract with chi_sim+eng; no network"]
    async fn pdf_and_ocr_extract_real_local_files() {
        let text = extract(
            "application/pdf",
            include_bytes!("../../tests/fixtures/osint/text.pdf"),
        )
        .await
        .unwrap();
        assert_eq!(text["pages"].as_array().unwrap().len(), 2);
        assert_eq!(text["pages"][1]["method"], "pdf_text");
        assert!(
            text["pages"][1]["text"]
                .as_str()
                .unwrap()
                .contains("Page 2")
        );
        for (media, bytes) in [
            (
                "image/png",
                include_bytes!("../../tests/fixtures/osint/scan.png").as_slice(),
            ),
            (
                "application/pdf",
                include_bytes!("../../tests/fixtures/osint/scan.pdf").as_slice(),
            ),
        ] {
            let output = extract(media, bytes).await.unwrap();
            assert_eq!(output["pages"][0]["method"], "ocr");
            let text = output["pages"][0]["text"].as_str().unwrap();
            assert!(text.contains("125"), "{text}");
            assert!(text.contains("Paris"), "{text}");
            assert!(text.contains("样本"), "{text}");
        }
        let oversized = extract(
            "application/pdf",
            include_bytes!("../../tests/fixtures/osint/too-many-pages.pdf"),
        )
        .await
        .unwrap_err();
        assert!(oversized.to_string().contains("1–20 页"));
        assert!(
            extract("application/pdf", b"%PDF-1.7\nbroken")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn extraction_keeps_literal_text_and_hashes() {
        let value = extract("text/plain", "<script>原件 & 125 人</script>".as_bytes())
            .await
            .unwrap();
        assert_eq!(value["pages"][0]["text"], "<script>原件 & 125 人</script>");
        assert_eq!(value["pages"][0]["method"], "utf8");
        assert!(extract("text/plain", b"   ").await.is_err());
        assert_eq!(
            page(1, "utf8", &"文".repeat(MAX_PAGE_CHARS + 1))["truncated"],
            true
        );
    }
}
