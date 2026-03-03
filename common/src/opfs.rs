//! Wrapper around the Origin Private File System (OPFS) API.
// TODO: missing error handling

use js_sys::Uint8Array;
use wasm_bindgen::{JsCast, JsValue, UnwrapThrowExt};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    File, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemWritableFileStream, StorageManager,
};

/// Directory handle in the OPFS
pub struct OPFSDir(FileSystemDirectoryHandle);

impl OPFSDir {
    fn from_js_value(obj: JsValue) -> Self {
        OPFSDir(obj.unchecked_into::<FileSystemDirectoryHandle>())
    }

    /// Open a subdirectory in this directory.
    pub async fn open_dir(&self, name: &str, create: bool) -> OPFSDir {
        let opts = FileSystemGetDirectoryOptions::new();
        opts.set_create(create);
        let promise = self.0.get_directory_handle_with_options(name, &opts);
        let res = JsFuture::from(promise).await.unwrap_throw();
        OPFSDir::from_js_value(res)
    }

    /// Open a file in this directory.
    pub async fn open_file(&self, name: &str, create: bool) -> OPFSFile {
        let opts = FileSystemGetFileOptions::new();
        opts.set_create(create);
        let promise = self.0.get_file_handle_with_options(name, &opts);
        let res = JsFuture::from(promise).await.unwrap_throw();
        OPFSFile::from_js_value(res)
    }

    /// List the entries in this directory.
    pub async fn list_entries(&self) -> Vec<String> {
        let mut entries = vec![];
        let entries_iter = self.0.keys();
        loop {
            let next_promise = entries_iter.next().unwrap_throw();
            let next_res = JsFuture::from(next_promise).await.unwrap_throw();
            let done = js_sys::Reflect::get(&next_res, &"done".into())
                .unwrap_throw()
                .as_bool()
                .expect("done should be a boolean");
            if done {
                break;
            }
            let entry = js_sys::Reflect::get(&next_res, &"value".into())
                .unwrap_throw()
                .as_string()
                .expect("entry should be a string");
            entries.push(entry);
        }
        entries
    }
}

/// File handle in the OPFS
pub struct OPFSFile(FileSystemFileHandle);

impl OPFSFile {
    fn from_js_value(obj: JsValue) -> Self {
        OPFSFile(obj.unchecked_into::<FileSystemFileHandle>())
    }

    async fn file(&self) -> File {
        let promise = self.0.get_file();
        let res = JsFuture::from(promise).await.unwrap_throw();
        res.unchecked_into::<File>()
    }

    /// Read the entire contents of the file.
    pub async fn read(&self) -> Vec<u8> {
        let file = self.file().await;
        let array_promise = file.array_buffer();
        let array_res = JsFuture::from(array_promise).await.unwrap_throw();
        let array = Uint8Array::new(&array_res);
        array.to_vec()
    }

    /// Get the name of the file.
    pub async fn name(&self) -> String {
        let file = self.file().await;
        file.name()
    }

    /// Write data to the file, replacing any existing contents.
    pub async fn write(&self, data: &[u8]) {
        let uint8_array = Uint8Array::from(data);
        let promise = self.0.create_writable();
        let res = JsFuture::from(promise).await.unwrap_throw();
        let writable = res.unchecked_into::<FileSystemWritableFileStream>();
        let write_promise = writable.write_with_js_u8_array(&uint8_array).unwrap_throw();
        JsFuture::from(write_promise).await.unwrap_throw();
        let close_promise = writable.close();
        JsFuture::from(close_promise).await.unwrap_throw();
    }
}

fn storage() -> StorageManager {
    let global = js_sys::global();
    let navigator = js_sys::Reflect::get(&global, &"navigator".into())
        .unwrap_throw()
        .unchecked_into::<web_sys::Navigator>();
    navigator.storage()
}

/// Request persistent storage for OPFS.
///
/// Does not run in Web Workers.
pub async fn persist() -> bool {
    let promise = storage().persist().unwrap_throw();
    let res = JsFuture::from(promise).await.unwrap_throw();
    res.as_bool().expect("persist() should return a boolean")
}

/// Get the root directory of the OPFS.
pub async fn root() -> OPFSDir {
    let promise = storage().get_directory();
    let res = JsFuture::from(promise).await.unwrap_throw();
    OPFSDir::from_js_value(res)
}

/// Open a directory at the given path, optionally creating it if it doesn't exist.
pub async fn open_dir(path: &str, create: bool) -> OPFSDir {
    let mut dir = root().await;
    for part in path.split('/').filter(|s| !s.is_empty()) {
        dir = dir.open_dir(part, create).await;
    }
    dir
}

/// Open a file at the given path, optionally creating it if it doesn't exist.
pub async fn open_file(path: &str, create: bool) -> OPFSFile {
    let mut parts = path.split('/').filter(|s| !s.is_empty());
    let filename = parts.next_back().expect("path should not be empty");
    let mut dir = root().await;
    for part in parts {
        dir = dir.open_dir(part, create).await;
    }
    dir.open_file(filename, create).await
}
