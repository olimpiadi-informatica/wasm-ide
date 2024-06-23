use std::{collections::HashMap, io::Read, sync::Arc};

use anyhow::{bail, Context, Result};
use brotli::BrotliDecompress;
use common::Language;
use gloo_timers::future::TimeoutFuture;
use log::info;
use tracing::warn;
use url::Url;
use wasmer::IntoBytes;

use crate::{
    instrument::instrument_binary,
    thread,
    wasi::{Executable, Fs},
};

#[allow(dead_code)]
pub struct ExecutionOutcome {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub struct RunnerInterface {
    pub should_stop: fn() -> bool,
    pub send_stdout: fn(&[u8]),
    pub send_stderr: fn(&[u8]),
    pub send_compiler_message: fn(&[u8]),
    pub send_done: fn(),
    pub send_compilation_done: fn(),
    pub send_error: fn(String),
    pub get_fs: fn(Language) -> Result<Fs>,
}

async fn fetch_tarbr(lang: Language, base_url: String) -> Result<Vec<u8>> {
    let tar_lang = match lang {
        Language::C | Language::CPP => "cpp",
        Language::Python => "python",
    };
    let url = Url::parse(&base_url)?.join(&format!("./compilers/{tar_lang}.tar.br"))?;
    Ok(reqwest::get(url)
        .await
        .context("Error fetching the compiler")?
        .bytes()
        .await?
        .to_vec())
}

async fn compile(
    lang: Language,
    source: String,
    fs: &Fs,
    interface: &RunnerInterface,
) -> Result<Executable> {
    let compile_c_or_cpp = {
        let source = source.clone();
        |lang_str: &'static [u8], std: &'static [u8]| async move {
            let exe_compile = Executable {
                fs: fs.clone(),
                args: vec![
                    b"clang++".to_vec(),
                    b"-cc1".to_vec(),
                    b"-isysroot".to_vec(),
                    b"/".to_vec(),
                    b"-I/include/c++/15.0.0/wasm32-wasi/".to_vec(),
                    b"-I/include/c++/15.0.0/".to_vec(),
                    b"-stdlib=libstdc++".to_vec(),
                    b"-internal-isystem".to_vec(),
                    b"/lib/clang/19/include".to_vec(),
                    b"-internal-isystem".to_vec(),
                    b"/include/wasm32-wasi-threads".to_vec(),
                    b"-I/include/".to_vec(),
                    b"-resource-dir".to_vec(),
                    b"lib/clang/19".to_vec(),
                    b"-target-feature".to_vec(),
                    b"+atomics".to_vec(),
                    b"-target-feature".to_vec(),
                    b"+bulk-memory".to_vec(),
                    b"-target-feature".to_vec(),
                    b"+mutable-globals".to_vec(),
                    b"-I.".to_vec(),
                    b"-x".to_vec(),
                    lang_str.to_vec(),
                    b"-O2".to_vec(),
                    b"-Wall".to_vec(),
                    std.to_vec(),
                    b"-emit-obj".to_vec(),
                    b"-".to_vec(),
                    b"-o".to_vec(),
                    b"-".to_vec(),
                ],
                exe: fs
                    .get_file(fs.get(fs.root(), b"bin/clang++").unwrap())
                    .unwrap()
                    .clone(),
                env: vec![],
                well_known_binary: Some("clang++"),
            };
            let ExecutionOutcome {
                stdout: obj,
                stderr: _,
            } = exe_compile
                .run(
                    source.into_bytes(),
                    interface.should_stop,
                    None,
                    None,
                    Some(interface.send_compiler_message),
                    None,
                )
                .await
                .context("Failed to compile source")?;

            let mut link_fs = fs.clone();
            link_fs.add_file(link_fs.root(), b"source.obj", Arc::new(obj));
            let exe_link = Executable {
                env: vec![],
                exe: fs
                    .get_file(fs.get(fs.root(), b"bin/wasm-ld").unwrap())
                    .unwrap()
                    .clone(),
                fs: link_fs,
                args: vec![
                    b"wasm-ld".to_vec(),
                    b"-L/lib/wasm32-wasi-threads/".to_vec(),
                    b"-lc".to_vec(),
                    b"/lib/clang/19/lib/wasi/libclang_rt.builtins-wasm32.a".to_vec(),
                    b"/lib/wasm32-wasi-threads/crt1.o".to_vec(),
                    b"-L/lib".to_vec(),
                    b"-lstdc++".to_vec(),
                    b"-lsupc++".to_vec(),
                    b"-z".to_vec(),
                    b"stack-size=16777216".to_vec(),
                    b"--stack-first".to_vec(),
                    b"--shared-memory".to_vec(),
                    b"--import-memory".to_vec(),
                    b"--export-memory".to_vec(),
                    b"--max-memory=4294967296".to_vec(),
                    b"-o".to_vec(),
                    b"-".to_vec(),
                    b"/source.obj".to_vec(),
                ],
                well_known_binary: Some("wasm-ld"),
            };

            let ExecutionOutcome {
                stdout: exe,
                stderr: _,
            } = exe_link
                .run(
                    vec![],
                    interface.should_stop,
                    None,
                    None,
                    Some(interface.send_compiler_message),
                    None,
                )
                .await
                .context("Failed to link source")?;

            let exe = Executable {
                args: vec![b"a.out".to_vec()],
                env: vec![],
                fs: Fs::new(),
                exe: Arc::new(exe),
                well_known_binary: None,
            };

            Ok(exe)
        }
    };
    match lang {
        Language::Python => {
            let mut new_fs = fs.clone();
            new_fs.add_file(fs.root(), b"solution.py", Arc::new(source.into_bytes()));
            let exe = fs
                .get_file(fs.get(fs.root(), b"bin/python3.12.wasm").unwrap())
                .unwrap();
            let exe = Executable {
                exe: exe.clone(),
                args: vec![b"/bin/python3.12.wasm".to_vec(), b"/solution.py".to_vec()],
                env: vec![(b"PYTHONHOME".to_vec(), b"/".to_vec())],
                fs: new_fs,
                well_known_binary: Some("python3.12"),
            };
            Ok(exe)
        }
        Language::C => compile_c_or_cpp(b"c", b"-std=c17").await,
        Language::CPP => compile_c_or_cpp(b"c++", b"-std=c++20").await,
    }
}

async fn get_fs(lang: Language, base_url: String) -> Result<Fs> {
    let body = fetch_tarbr(lang, base_url).await?;
    let mut dec_body = vec![];
    BrotliDecompress(&mut &body[..], &mut dec_body)?;
    let mut reader = &dec_body[..];
    let mut files = tar::Archive::new(&mut reader);
    let files = files
        .entries()?
        .map(|x| {
            let mut x = x?;
            let name = x
                .path()?
                .to_string_lossy()
                .into_bytes()
                .strip_prefix(b".")
                .expect("invalid tarball")
                .to_vec();
            let mut contents = vec![];
            x.read_to_end(&mut contents)?;
            Ok((name, Arc::new(contents)))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Fs::from_files(files))
}

pub async fn prepare_cache(
    base_url: String,
    language: Language,
    cache: &mut HashMap<Language, Fs>,
) -> Result<Language> {
    info!("work started");
    if !cache.contains_key(&language) {
        info!("fetching compiler");
        cache.insert(language, get_fs(language, base_url).await?);
    }
    Ok(language)
}

async fn compile_one_inner(
    source: String,
    language: Language,
    input: Vec<u8>,
    interface: RunnerInterface,
) -> Result<()> {
    let files = (interface.get_fs)(language).unwrap();
    info!("compiling");
    let executable = compile(language, source, &files, &interface).await?;
    (interface.send_compilation_done)();
    info!("running");
    executable
        .run(
            input,
            interface.should_stop,
            None,
            Some(interface.send_stdout),
            Some(interface.send_stderr),
            None,
        )
        .await
        .context("Failed to run solution")?;
    Ok(())
}

pub async fn compile_one(
    source: String,
    language: Language,
    input: Vec<u8>,
    interface: RunnerInterface,
) {
    let send_done = interface.send_done;
    let send_error = interface.send_error;
    match compile_one_inner(source, language, input, interface).await {
        Ok(_) => send_done(),
        Err(e) => send_error(format!("{:?}", e)),
    }
}

pub struct LSInterface {
    pub should_stop: fn() -> bool,
    pub recv_stdin: fn(&mut [u8]) -> usize,
    pub send_stdout: fn(&[u8]),
    pub send_stderr: fn(&[u8]),
    pub get_fs: fn(Language) -> Result<Fs>,
    pub notify: fn(),
}

pub async fn start_language_server(language: Language, interface: LSInterface) -> Result<()> {
    let mut fs = (interface.get_fs)(language).unwrap();
    let language_server = match language {
        Language::CPP => {
            fs.add_file(
                fs.root(),
                b"compile_flags.txt",
                Arc::new(
                    br#"
-Wall
-O2
-I/include/c++/15.0.0/
-I/include/c++/15.0.0/wasm32-wasi/
-resource-dir=/lib/clang/19
-std=c++20
"#
                    .to_vec(),
                ),
            );

            // As clang++ takes quite a while to complete instrumentation (~7-8s), try to start
            // instrumentation in the background after the LS starts.
            if let Ok(f) = fs.get(fs.root(), b"bin/clang++") {
                if let Ok(f) = fs.get_file(f) {
                    let f = f.clone();
                    thread::spawn_simple(move || {
                        if let Err(e) = instrument_binary(&f, Some("clang++")) {
                            warn!("error during pre-instrumentation of clang++: {e}");
                        };
                    })
                    .await;
                }
            }

            Executable {
                fs: fs.clone(),
                args: vec![b"clangd".to_vec(), b"--pch-storage=memory".to_vec()],
                exe: fs
                    .get_file(fs.get(fs.root(), b"bin/clangd").unwrap())
                    .unwrap()
                    .clone(),
                env: vec![],
                well_known_binary: Some("clangd"),
            }
        }
        _ => {
            bail!("lsp not yet supported")
        }
    };
    let mut sleep_time = 10;
    loop {
        let result = language_server
            .run(
                vec![],
                interface.should_stop,
                Some(interface.recv_stdin),
                Some(interface.send_stdout),
                Some(interface.send_stderr),
                Some(interface.notify),
            )
            .await;
        match result {
            Ok(_) => break,
            Err(e) => {
                warn!("LS error: {e}");
                TimeoutFuture::new(sleep_time).await;
                sleep_time = sleep_time.saturating_mul(2);
            }
        }
    }
    Ok(())
}
