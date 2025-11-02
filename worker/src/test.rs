use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Once;

use common::init_logging;
use serde::Deserialize;
use wasm_bindgen_test::*;

use crate::os::{FdEntry, FsEntry, ProcessHandle, StatusCode};
use crate::util::fs_from_tar;

wasm_bindgen_test_configure!(run_in_dedicated_worker);

#[derive(Default, Deserialize)]
struct Config {
    #[serde(default)]
    args: Vec<String>,

    #[serde(default)]
    dirs: Vec<String>,

    #[serde(default)]
    env: HashMap<String, String>,

    #[serde(default)]
    exit_code: u32,

    #[serde(default)]
    stdout: String,
    // wasi_functions: Vec<String>,
}

async fn test(testsuite: &[u8]) {
    static LOGGING_INIT: Once = Once::new();
    LOGGING_INIT.call_once(|| {
        init_logging();
    });

    let fs = fs_from_tar(testsuite).unwrap();
    let FsEntry::Dir(root_dir) = &fs.entries[fs.root() as usize] else {
        panic!();
    };

    for file in root_dir.keys() {
        let Some(basename) = file.strip_suffix(b".wasm") else {
            continue;
        };

        tracing::info!("Running: {:?}", String::from_utf8_lossy(file));

        let mut json = basename.to_vec();
        json.extend(b".json");

        let config = fs
            .get_file_with_path(&json)
            .ok()
            .map(|data| serde_json::from_slice::<Config>(&data).unwrap())
            .unwrap_or_default();

        let proc = ProcessHandle::builder()
            .fs(fs.clone())
            .stdin(FdEntry::Data {
                data: Vec::new(),
                offset: 0,
            })
            .stdout(FdEntry::Data {
                data: Vec::new(),
                offset: 0,
            })
            .stderr(FdEntry::WriteFn(Rc::new(move |buf: &[u8]| {
                tracing::info!("stderr: {:?}", String::from_utf8_lossy(buf));
                buf.len()
            })))
            ._envs(
                config
                    .env
                    .into_iter()
                    .map(|(k, v)| [k.as_bytes(), v.as_bytes()].join(&b'=')),
            )
            ._preopens(config.dirs.into_iter().map(String::into_bytes).collect())
            .arg(file.clone())
            .args(config.args)
            .spawn_with_path(file);

        let status_code = proc.proc.wait().await;
        assert_eq!(status_code, StatusCode::Exited(config.exit_code));

        let stdout = proc.proc.inner.borrow_mut().fds[1]
            .take()
            .unwrap()
            .into_data()
            .ok()
            .unwrap()
            .0;
        assert_eq!(stdout, config.stdout.as_bytes());
    }
}

#[wasm_bindgen_test]
async fn test_as() {
    const TESTSUITE_AS: &[u8] = include_bytes!("../../testsuite/as.tar");
    test(TESTSUITE_AS).await;
}

#[wasm_bindgen_test]
async fn test_c() {
    const TESTSUITE_C: &[u8] = include_bytes!("../../testsuite/c.tar");
    test(TESTSUITE_C).await;
}

// TODO: enable after fixing the issues
//#[wasm_bindgen_test]
//async fn test_rs() {
//    const TESTSUITE_RS: &[u8] = include_bytes!("../../testsuite/rs.tar");
//    test(TESTSUITE_RS).await;
//}
