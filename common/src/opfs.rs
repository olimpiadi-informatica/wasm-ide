//! Wrapper around the Origin Private File System (OPFS) API.
// TODO: missing error handling

use gloo_utils::window;
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
    window().navigator().storage()
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
