// Copyright 2018-2019 the Deno authors. All rights reserved. MIT license.
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
#[macro_use]
extern crate futures;
#[macro_use]
extern crate serde_json;
extern crate clap;
extern crate deno;

mod ansi;
pub mod compiler;
pub mod deno_dir;
pub mod errors;
pub mod flags;
mod fs;
mod global_timer;
mod http_body;
mod http_util;
pub mod js_errors;
pub mod modules;
pub mod msg;
pub mod msg_util;
pub mod ops;
pub mod permissions;
mod repl;
pub mod resolve_addr;
pub mod resources;
mod startup_data;
pub mod state;
mod tokio_util;
mod tokio_write;
pub mod version;
pub mod worker;

use crate::errors::RustOrJsError;
use crate::state::ThreadSafeState;
use crate::worker::Worker;
use futures::lazy;
use futures::Future;
use log::{LevelFilter, Metadata, Record};
use std::env;

static LOGGER: Logger = Logger;

struct Logger;

impl log::Log for Logger {
  fn enabled(&self, metadata: &Metadata) -> bool {
    metadata.level() <= log::max_level()
  }

  fn log(&self, record: &Record) {
    if self.enabled(record.metadata()) {
      println!("{} RS - {}", record.level(), record.args());
    }
  }
  fn flush(&self) {}
}

fn print_err_and_exit(err: RustOrJsError) {
  eprintln!("{}", err.to_string());
  std::process::exit(1);
}

fn js_check<E>(r: Result<(), E>)
where
  E: Into<RustOrJsError>,
{
  if let Err(err) = r {
    print_err_and_exit(err.into());
  }
}

fn main() {
  #[cfg(windows)]
  ansi_term::enable_ansi_support().ok(); // For Windows 10

  log::set_logger(&LOGGER).unwrap();
  let args = env::args().collect();
  let (mut flags, mut rest_argv) =
    flags::set_flags(args).unwrap_or_else(|err| {
      eprintln!("{}", err);
      std::process::exit(1)
    });

  log::set_max_level(if flags.log_debug {
    LevelFilter::Debug
  } else {
    LevelFilter::Warn
  });

  if flags.fmt {
    rest_argv.insert(1, "https://deno.land/std/prettier/main.ts".to_string());
    flags.allow_read = true;
    flags.allow_write = true;
  }

  let should_prefetch = flags.prefetch || flags.info;
  let should_display_info = flags.info;

  let state = ThreadSafeState::new(flags, rest_argv, ops::op_selector_std);
  let mut main_worker = Worker::new(
    "main".to_string(),
    startup_data::deno_isolate_init(),
    state.clone(),
  );

  let main_future = lazy(move || {
    // Setup runtime.
    js_check(main_worker.execute("denoMain()"));

    if state.flags.eval {
      // Wrap provided script in async function so asynchronous methods
      // work. This is required until top-level await is not supported.
      let js_source = format!(
        "async function _topLevelWrapper(){{
          {}
        }}
        _topLevelWrapper();
        ",
        &state.argv[1]
      );
      // ATM imports in `deno eval` are not allowed
      // TODO Support ES modules once Worker supports evaluating anonymous modules.
      js_check(main_worker.execute(&js_source));
    } else {
      // Execute main module.
      if let Some(main_module) = state.main_module() {
        debug!("main_module {}", main_module);
        js_check(main_worker.execute_mod(&main_module, should_prefetch));
        if should_display_info {
          // Display file info and exit. Do not run file
          main_worker.print_file_info(&main_module);
          std::process::exit(0);
        }
      }
    }

    main_worker.then(|result| {
      js_check(result);
      Ok(())
    })
  });

  tokio_util::run(main_future);
}
