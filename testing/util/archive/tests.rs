use async_zip::ZipEntryBuilder;
use async_zip::base::write::ZipFileWriter;
use futures::{AsyncSeek, AsyncWriteExt};
use smol::io::Cursor;
use tempfile::TempDir;

use super::*;

#[allow(unused_variables)]
async fn compress_zip(src_dir: &Path, dst: &Path, keep_file_permissions: bool) -> Result<()> {
    let mut out = smol::fs::File::create(dst).await?;
    let mut writer = ZipFileWriter::new(&mut out);

    for entry in walkdir::WalkDir::new(src_dir) {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            continue;
        }

        let relative_path = path.strip_prefix(src_dir)?;
        let data = smol::fs::read(&path).await?;

        let filename = relative_path.display().to_string();

        #[cfg(unix)]
        {
            let mut builder = ZipEntryBuilder::new(filename.into(), async_zip::Compression::Stored);
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(path)?;
            let perms = keep_file_permissions.then(|| metadata.permissions().mode() as u16);
            builder = builder.unix_permissions(perms.unwrap_or_default());
            writer.write_entry_whole(builder, &data).await?;
        }
        #[cfg(not(unix))]
        {
            let builder = ZipEntryBuilder::new(filename.into(), async_zip::Compression::Stored);
            writer.write_entry_whole(builder, &data).await?;
        }
    }

    writer.close().await?;
    out.flush().await?;
    out.sync_all().await?;

    Ok(())
}

#[track_caller]
fn assert_file_content(path: &Path, content: &str) {
    assert!(path.exists(), "file not found: {:?}", path);
    let actual = std::fs::read_to_string(path).unwrap();
    assert_eq!(actual, content);
}

#[track_caller]
fn make_test_data() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path();

    std::fs::write(dst.join("test"), "Hello world.").unwrap();
    std::fs::create_dir_all(dst.join("foo/bar")).unwrap();
    std::fs::write(dst.join("foo/bar.txt"), "Foo bar.").unwrap();
    std::fs::write(dst.join("foo/dar.md"), "Bar dar.").unwrap();
    std::fs::write(dst.join("foo/bar/dar你好.txt"), "你好世界").unwrap();

    dir
}

async fn read_archive(path: &Path) -> impl AsyncRead + AsyncSeek + Unpin {
    let data = smol::fs::read(&path).await.unwrap();
    Cursor::new(data)
}

#[test]
fn test_extract_zip() {
    let test_dir = make_test_data();
    let zip_file = test_dir.path().join("test.zip");

    smol::block_on(async {
        compress_zip(test_dir.path(), &zip_file, true)
            .await
            .unwrap();
        let reader = read_archive(&zip_file).await;

        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path();
        extract_zip(dst, reader).await.unwrap();

        assert_file_content(&dst.join("test"), "Hello world.");
        assert_file_content(&dst.join("foo/bar.txt"), "Foo bar.");
        assert_file_content(&dst.join("foo/dar.md"), "Bar dar.");
        assert_file_content(&dst.join("foo/bar/dar你好.txt"), "你好世界");
    });
}

#[cfg(unix)]
#[test]
fn test_extract_zip_preserves_executable_permissions() {
    use std::os::unix::fs::PermissionsExt;

    smol::block_on(async {
        let test_dir = tempfile::tempdir().unwrap();
        let executable_path = test_dir.path().join("my_script");

        // Create an executable file
        std::fs::write(&executable_path, "#!/bin/bash\necho 'Hello'").unwrap();
        let mut perms = std::fs::metadata(&executable_path).unwrap().permissions();
        perms.set_mode(0o755); // rwxr-xr-x
        std::fs::set_permissions(&executable_path, perms).unwrap();

        // Create zip
        let zip_file = test_dir.path().join("test.zip");
        compress_zip(test_dir.path(), &zip_file, true)
            .await
            .unwrap();

        // Extract to new location
        let extract_dir = tempfile::tempdir().unwrap();
        let reader = read_archive(&zip_file).await;
        extract_zip(extract_dir.path(), reader).await.unwrap();

        // Check permissions are preserved
        let extracted_path = extract_dir.path().join("my_script");
        assert!(extracted_path.exists());
        let extracted_perms = std::fs::metadata(&extracted_path).unwrap().permissions();
        assert_eq!(extracted_perms.mode() & 0o777, 0o755);
    });
}

#[cfg(unix)]
#[test]
fn test_extract_zip_sets_default_permissions() {
    use std::os::unix::fs::PermissionsExt;

    smol::block_on(async {
        let test_dir = tempfile::tempdir().unwrap();
        let file_path = test_dir.path().join("my_script");

        std::fs::write(&file_path, "#!/bin/bash\necho 'Hello'").unwrap();
        // The permissions will be shaped by the umask in the test environment
        let original_perms = std::fs::metadata(&file_path).unwrap().permissions();

        // Create zip
        let zip_file = test_dir.path().join("test.zip");
        compress_zip(test_dir.path(), &zip_file, false)
            .await
            .unwrap();

        // Extract to new location
        let extract_dir = tempfile::tempdir().unwrap();
        let reader = read_archive(&zip_file).await;
        extract_zip(extract_dir.path(), reader).await.unwrap();

        // Permissions were not stored, so will be whatever the umask generates
        // by default for new files. This should match what we saw when we previously wrote
        // the file.
        let extracted_path = extract_dir.path().join("my_script");
        assert!(extracted_path.exists());
        let extracted_perms = std::fs::metadata(&extracted_path).unwrap().permissions();
        assert_eq!(
            extracted_perms.mode(),
            original_perms.mode(),
            "Expected matching Unix file mode for unzipped file without keep_file_permissions"
        );
        assert_eq!(
            extracted_perms, original_perms,
            "Expected default set of permissions for unzipped file without keep_file_permissions"
        );
    });
}

#[test]
fn test_archive_path_is_normal_rejects_traversal() {
    assert!(!archive_path_is_normal("../parent.txt"));
    assert!(!archive_path_is_normal("foo/../../grandparent.txt"));
    assert!(!archive_path_is_normal("/tmp/absolute.txt"));

    assert!(archive_path_is_normal("foo/bar.txt"));
    assert!(archive_path_is_normal("foo/bar/baz.txt"));
    assert!(archive_path_is_normal("./foo/bar.txt"));
    assert!(archive_path_is_normal("normal.txt"));
}

async fn build_zip_with_entries(entries: &[(&str, &[u8])]) -> Cursor<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    let mut writer = ZipFileWriter::new(&mut buf);
    for (name, data) in entries {
        let builder = ZipEntryBuilder::new((*name).into(), async_zip::Compression::Stored);
        writer.write_entry_whole(builder, data).await.unwrap();
    }
    writer.close().await.unwrap();
    buf.set_position(0);
    buf
}

#[test]
fn test_extract_zip_skips_path_traversal_entries() {
    smol::block_on(async {
        let base_dir = tempfile::tempdir().unwrap();
        let extract_dir = base_dir.path().join("subdir");
        std::fs::create_dir_all(&extract_dir).unwrap();

        let absolute_target = base_dir.path().join("absolute.txt");
        let reader = build_zip_with_entries(&[
            ("normal.txt", b"normal file"),
            ("subdir/nested.txt", b"nested file"),
            ("../parent.txt", b"parent file"),
            ("foo/../../grandparent.txt", b"grandparent file"),
            (absolute_target.to_str().unwrap(), b"absolute file"),
        ])
        .await;

        extract_zip(&extract_dir, reader).await.unwrap();

        assert_file_content(&extract_dir.join("normal.txt"), "normal file");
        assert_file_content(&extract_dir.join("subdir/nested.txt"), "nested file");

        assert!(
            !base_dir.path().join("parent.txt").exists(),
            "parent traversal entry should have been skipped"
        );
        assert!(
            !base_dir.path().join("grandparent.txt").exists(),
            "nested traversal entry should have been skipped"
        );
        assert!(
            !absolute_target.exists(),
            "absolute path entry should have been skipped"
        );
    });
}
