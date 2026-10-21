//! Validate actual write buffers, including patches recomputed after approval.

use std::io;
use std::sync::Arc;

use codex_config::output_filter::OutputFilter;
use codex_exec_server::CopyOptions;
use codex_exec_server::CreateDirectoryOptions;
use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::ExecutorFileSystemFuture;
use codex_exec_server::FileMetadata;
use codex_exec_server::FileSystemReadStream;
use codex_exec_server::FileSystemSandboxContext;
use codex_exec_server::GetMetadataOptions;
use codex_exec_server::ReadDirectoryEntry;
use codex_exec_server::ReadFileOptions;
use codex_exec_server::RemoveOptions;
use codex_exec_server::WalkOptions;
use codex_exec_server::WalkOutcome;
use codex_exec_server::WriteFileOptions;
use codex_utils_path_uri::PathUri;

pub(crate) fn guarded(inner: Arc<dyn ExecutorFileSystem>) -> Arc<dyn ExecutorFileSystem> {
    match codex_config::output_filter::global() {
        Some(filter) => Arc::new(FilteredFileSystem { inner, filter }),
        None => inner,
    }
}

struct FilteredFileSystem<'filter> {
    inner: Arc<dyn ExecutorFileSystem>,
    filter: &'filter OutputFilter,
}

fn blocked() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "The global output filter blocked this filesystem operation.",
    )
}

impl ExecutorFileSystem for FilteredFileSystem<'_> {
    fn canonicalize<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, PathUri> {
        self.inner.canonicalize(path, sandbox)
    }

    fn read_file<'a>(
        &'a self,
        path: &'a PathUri,
        options: ReadFileOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<u8>> {
        self.inner.read_file(path, options, sandbox)
    }

    fn read_file_stream<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSystemReadStream> {
        self.inner.read_file_stream(path, sandbox)
    }

    fn write_file<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        options: WriteFileOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(async move {
            let text = std::str::from_utf8(&contents).map_err(|_| blocked())?;
            if self.filter.is_match(text) || self.filter.is_match(&path.to_string()) {
                return Err(blocked());
            }
            self.inner
                .write_file(path, contents, options, sandbox)
                .await
        })
    }

    fn create_directory<'a>(
        &'a self,
        path: &'a PathUri,
        options: CreateDirectoryOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(async move {
            if self.filter.is_match(&path.to_string()) {
                return Err(blocked());
            }
            self.inner.create_directory(path, options, sandbox).await
        })
    }

    fn get_metadata<'a>(
        &'a self,
        path: &'a PathUri,
        options: GetMetadataOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileMetadata> {
        self.inner.get_metadata(path, options, sandbox)
    }

    fn read_directory<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<ReadDirectoryEntry>> {
        self.inner.read_directory(path, sandbox)
    }

    fn walk<'a>(
        &'a self,
        path: &'a PathUri,
        options: WalkOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, WalkOutcome> {
        self.inner.walk(path, options, sandbox)
    }

    fn remove<'a>(
        &'a self,
        path: &'a PathUri,
        options: RemoveOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        self.inner.remove(path, options, sandbox)
    }

    fn copy<'a>(
        &'a self,
        _source: &'a PathUri,
        _destination: &'a PathUri,
        _options: CopyOptions,
        _sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        Box::pin(async { Err(blocked()) })
    }
    // The default streaming-write implementation is unsupported. Never forward
    // unvalidated chunks; a match could span any number of them.
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_config::output_filter::OutputFilterConfig;
    use codex_config::output_filter::OutputFilterRule;
    use codex_exec_server::LOCAL_FS;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn rejected_actual_write_preserves_the_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("output.txt");
        let path = PathUri::from_host_native_path(&file).unwrap();
        let filter = OutputFilter::compile(OutputFilterConfig {
            rules: vec![OutputFilterRule {
                pattern: "secret".into(),
                exempt_pattern: None,
                replacement: "[filtered]".into(),
            }],
        })
        .unwrap();
        let fs = FilteredFileSystem {
            inner: LOCAL_FS.clone(),
            filter: &filter,
        };
        fs.write_file(&path, b"safe".to_vec(), Default::default(), None)
            .await
            .unwrap();
        let result = fs
            .write_file(&path, b"secret".to_vec(), Default::default(), None)
            .await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "safe");
        assert!(
            fs.write_file(&path, vec![255], Default::default(), None)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "safe");
    }
}
