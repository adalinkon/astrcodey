//! Tool result artifact file helpers.

use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, ErrorKind, Read, Write},
    path::Path,
};

use astrcode_core::storage::{
    BackgroundTaskOutputSlice, ToolResultArtifactInput, ToolResultArtifactRef,
    ToolResultArtifactSlice,
};

/// 生成 artifact 文件名。
pub fn tool_result_file_name(tool_name: &str, call_id: &str) -> String {
    let safe_tool = sanitize_for_filename(tool_name);
    let safe_call = sanitize_for_filename(call_id);
    format!("{safe_tool}-{safe_call}.txt")
}

/// 写入工具结果 artifact 正文。
pub fn write_tool_result_file(
    dir: &Path,
    input: &ToolResultArtifactInput,
) -> std::io::Result<ToolResultArtifactRef> {
    std::fs::create_dir_all(dir)?;
    for suffix in 0..1000 {
        let file_name = tool_result_file_name_with_suffix(&input.tool_name, &input.call_id, suffix);
        let path = dir.join(file_name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(input.content.as_bytes())?;
                return Ok(ToolResultArtifactRef {
                    bytes: input.content.len(),
                    path: Some(path.display().to_string()),
                });
            },
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                if fs::read(&path)? == input.content.as_bytes() {
                    return Ok(ToolResultArtifactRef {
                        bytes: input.content.len(),
                        path: Some(path.display().to_string()),
                    });
                }
            },
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        "too many tool result artifact filename collisions",
    ))
}

/// 从 artifact 正文中读取一段字符切片。
pub fn slice_tool_result(
    path: &str,
    content: &str,
    char_offset: usize,
    max_chars: usize,
) -> ToolResultArtifactSlice {
    let mut iter = content.chars().skip(char_offset);
    let text: String = iter.by_ref().take(max_chars).collect();
    let returned_chars = text.chars().count();
    let has_more = iter.next().is_some();
    ToolResultArtifactSlice {
        path: path.to_string(),
        bytes: content.len(),
        char_offset,
        returned_chars,
        next_char_offset: has_more.then_some(char_offset.saturating_add(returned_chars)),
        has_more,
        content: text,
    }
}

fn sanitize_for_filename(input: &str) -> String {
    let sanitized = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect::<String>();
    if sanitized.is_empty() {
        "result".to_string()
    } else {
        sanitized
    }
}

/// 后台任务输出文件默认大小上限 (100 MB)。
pub const DEFAULT_BG_TASK_OUTPUT_LIMIT: usize = 100 * 1024 * 1024;

/// 写入后台任务输出到 `{dir}/{task_id}.output`。
///
/// `task_id` 被视为全局唯一，不做碰撞处理。
/// 如果文件已存在则覆盖（允许重写）。
///
/// 超过 `max_bytes` 时在 UTF-8 char boundary 处截断并追加截断提示。
pub fn write_background_task_file(
    dir: &Path,
    task_id: &str,
    content: &str,
    max_bytes: usize,
) -> std::io::Result<usize> {
    std::fs::create_dir_all(dir)?;
    let safe_id = sanitize_for_filename(task_id);
    let file_name = format!("{safe_id}.output");
    let path = dir.join(file_name);

    let (to_write, truncated) = if content.len() > max_bytes {
        let mut end = max_bytes;
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }
        (&content[..end], true)
    } else {
        (content, false)
    };

    let mut output = to_write.to_string();
    if truncated {
        let total_kb = content.len() / 1024;
        let kept_kb = to_write.len() / 1024;
        output.push_str(&format!(
            "\n\n[output truncated: {kept_kb}KB retained of {total_kb}KB total]"
        ));
    }

    std::fs::write(&path, output.as_bytes())?;
    Ok(to_write.len())
}

/// 小文件阈值 (10 MB)。低于此大小直接 `read_to_string`；
/// 高于此大小使用流式读取，避免全量加载到内存。
const SMALL_FILE_THRESHOLD: usize = 10 * 1024 * 1024;

/// 读取后台任务输出的分页切片。
///
/// 小文件直接加载；大文件使用流式分页，不全量加载到内存。
/// 文件不存在时返回 `io::ErrorKind::NotFound`。
pub fn read_background_task_file(
    dir: &Path,
    task_id: &str,
    char_offset: usize,
    max_chars: usize,
) -> std::io::Result<BackgroundTaskOutputSlice> {
    let safe_id = sanitize_for_filename(task_id);
    let file_name = format!("{safe_id}.output");
    let path = dir.join(file_name);

    let total_bytes = std::fs::metadata(&path)?.len() as usize;

    if total_bytes <= SMALL_FILE_THRESHOLD {
        read_background_task_small(&path, task_id, total_bytes, char_offset, max_chars)
    } else {
        read_background_task_large(&path, task_id, total_bytes, char_offset, max_chars)
    }
}

/// 小文件：全量加载后分页。
fn read_background_task_small(
    path: &Path,
    task_id: &str,
    total_bytes: usize,
    char_offset: usize,
    max_chars: usize,
) -> std::io::Result<BackgroundTaskOutputSlice> {
    let content = std::fs::read_to_string(path)?;
    let mut iter = content.chars().skip(char_offset);
    let text: String = iter.by_ref().take(max_chars).collect();
    let returned_chars = text.chars().count();
    let has_more = iter.next().is_some();
    Ok(BackgroundTaskOutputSlice {
        task_id: task_id.to_string(),
        bytes: total_bytes,
        char_offset,
        returned_chars,
        next_char_offset: has_more.then_some(char_offset.saturating_add(returned_chars)),
        has_more,
        content: text,
    })
}

/// 大文件：流式读取，不全量加载。
///
/// 使用 8KB 缓冲区扫描 char_offset 个字符以定位字节偏移，
/// 然后读取足够字节提取 max_chars 个字符。
fn read_background_task_large(
    path: &Path,
    task_id: &str,
    total_bytes: usize,
    char_offset: usize,
    max_chars: usize,
) -> std::io::Result<BackgroundTaskOutputSlice> {
    use std::io::{BufReader, Read, Seek, SeekFrom};

    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);

    // Phase 1: 跳过 char_offset 个字符，定位字节偏移
    let byte_start = if char_offset == 0 {
        0usize
    } else {
        skip_chars(&mut reader, char_offset)?
    };

    // Phase 2: 读取足够的字节来提取 max_chars 个字符
    // 每个字符最多 4 字节，但读多一点也不浪费——只是多 decode 几个 char
    let bytes_available = total_bytes.saturating_sub(byte_start);
    let read_size = (max_chars * 4).min(bytes_available).max(1);
    let mut buf = vec![0u8; read_size];

    // 先 seek 到起始位置（skip_chars 已经推进了 reader 位置）
    // 如果 skip_chars 精确跳过了 char_offset 个字符，reader 已在正确位置
    // 但为安全起见，显式 seek
    reader.seek(SeekFrom::Start(byte_start as u64))?;
    let n = reader.read(&mut buf)?;
    buf.truncate(n);

    // 解码为字符串（处理尾部可能的不完整 UTF-8）
    let text_raw = String::from_utf8_lossy(&buf);
    let mut chars = text_raw.chars();
    let text: String = chars.by_ref().take(max_chars).collect();
    let returned_chars = text.chars().count();

    // 判断是否还有更多内容
    let has_more = chars.next().is_some() || (byte_start + n < total_bytes);

    Ok(BackgroundTaskOutputSlice {
        task_id: task_id.to_string(),
        bytes: total_bytes,
        char_offset,
        returned_chars,
        next_char_offset: has_more.then_some(char_offset.saturating_add(returned_chars)),
        has_more,
        content: text,
    })
}

/// 从 BufReader 中跳过 N 个 UTF-8 字符，返回消耗的字节数。
fn skip_chars<R: Read>(reader: &mut BufReader<R>, count: usize) -> std::io::Result<usize> {
    let mut chars_seen = 0usize;
    let mut total_bytes = 0usize;

    while chars_seen < count {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            break;
        }
        // 找到有效的 UTF-8 边界
        let valid_str = match std::str::from_utf8(buf) {
            Ok(s) => s,
            Err(e) => std::str::from_utf8(&buf[..e.valid_up_to()]).unwrap_or(""),
        };
        if valid_str.is_empty() {
            // 跳过一个字节以避免死循环（非 UTF-8 字节）
            reader.consume(1);
            total_bytes += 1;
            continue;
        }
        let remaining = count - chars_seen;
        let mut local_chars = 0usize;
        let mut local_bytes = 0usize;
        for c in valid_str.chars() {
            if local_chars >= remaining {
                break;
            }
            local_chars += 1;
            local_bytes += c.len_utf8();
        }
        reader.consume(local_bytes);
        total_bytes += local_bytes;
        chars_seen += local_chars;
    }
    Ok(total_bytes)
}

fn tool_result_file_name_with_suffix(tool_name: &str, call_id: &str, suffix: usize) -> String {
    let base = tool_result_file_name(tool_name, call_id);
    if suffix == 0 {
        return base;
    }
    let stem = base.trim_end_matches(".txt");
    format!("{stem}-{suffix}.txt")
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn file_name_filters_path_segments() {
        assert_eq!(
            tool_result_file_name("shell/../../bad", "../call"),
            "shellbad-call.txt"
        );
    }

    #[test]
    fn writing_same_result_reuses_file_and_collision_uses_suffix() {
        let dir = unique_test_dir("tool-results");
        let input = ToolResultArtifactInput {
            call_id: "call-1".into(),
            tool_name: "shell".into(),
            content: "abcdef".into(),
        };

        let first = write_tool_result_file(&dir, &input).unwrap();
        let second = write_tool_result_file(&dir, &input).unwrap();
        assert_eq!(first.path, second.path);

        let changed = ToolResultArtifactInput {
            content: "changed".into(),
            ..input
        };
        let third = write_tool_result_file(&dir, &changed).unwrap();
        assert_ne!(first.path, third.path);

        let first_path = PathBuf::from(first.path.unwrap());
        let third_path = PathBuf::from(third.path.unwrap());
        assert_eq!(std::fs::read_to_string(first_path).unwrap(), "abcdef");
        assert_eq!(std::fs::read_to_string(third_path).unwrap(), "changed");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn slices_text_with_next_offset() {
        let slice = slice_tool_result("D:/sessions/session/tool-results/call.txt", "abcdef", 2, 3);

        assert_eq!(slice.content, "cde");
        assert_eq!(slice.next_char_offset, Some(5));
        assert!(slice.has_more);
    }

    #[test]
    fn write_background_task_file_truncates_at_char_boundary() {
        let dir = unique_test_dir("bg-task-write");
        let content = "a".repeat(10) + "🙂";
        let kept = write_background_task_file(&dir, "task-1", &content, 10).unwrap();
        assert_eq!(kept, 10);

        let written = std::fs::read_to_string(dir.join("task-1.output")).unwrap();
        assert_eq!(&written[..10], &"a".repeat(10));
        assert!(written.contains("[output truncated:"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn read_background_task_file_pages_small_file() {
        let dir = unique_test_dir("bg-task-read-small");
        let content: String = (0..500)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        write_background_task_file(&dir, "task-small", &content, content.len()).unwrap();

        let first = read_background_task_file(&dir, "task-small", 0, 100).unwrap();
        assert_eq!(first.returned_chars, 100);
        assert_eq!(first.content, content.chars().take(100).collect::<String>());
        assert!(first.has_more);

        let second =
            read_background_task_file(&dir, "task-small", first.next_char_offset.unwrap(), 100)
                .unwrap();
        assert_eq!(second.char_offset, 100);
        assert_eq!(second.returned_chars, 100);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn read_background_task_file_uses_streaming_path_for_large_files() {
        let dir = unique_test_dir("bg-task-read-large");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("task-large.output");
        let payload = vec![b'a'; SMALL_FILE_THRESHOLD + 128];
        std::fs::write(&path, &payload).unwrap();

        let slice = read_background_task_file(&dir, "task-large", 0, 64).unwrap();
        assert_eq!(slice.returned_chars, 64);
        assert_eq!(slice.content, "a".repeat(64));
        assert!(slice.has_more);
        assert_eq!(slice.bytes, payload.len());

        let _ = std::fs::remove_dir_all(dir);
    }

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()))
    }
}
