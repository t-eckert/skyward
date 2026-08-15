//! Make sure there is always something to embed.
//!
//! The client is compiled into the binary, which means `rust-embed` needs the
//! directory to exist at compile time. A fresh clone has never run
//! `npm run build`, and `cargo build` failing because of that would be a
//! miserable first experience — worse, it would fail with a path error that
//! says nothing about the actual problem.
//!
//! So if the client has not been built, write a single page that says exactly
//! that. The binary still compiles, `skyward run` still serves the API, and
//! opening it in a browser tells you the one command you are missing.

use std::path::Path;

fn main() {
    let client = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../client/build");

    // Rebuild when the client is rebuilt, so a stale UI cannot be baked into a
    // fresh binary. This is the whole reason the client is embedded rather than
    // served from disk: one artifact, one version, one way to deploy it.
    println!("cargo:rerun-if-changed={}", client.display());

    if client.join("index.html").exists() {
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&client) {
        println!("cargo:warning=could not create {}: {e}", client.display());
        return;
    }

    let placeholder = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>Skyward — client not built</title>
    <style>
      body { margin: 0; display: grid; place-items: center; height: 100vh;
             background: #05080b; color: #fff;
             font: 15px/1.6 ui-sans-serif, system-ui, sans-serif; }
      div { max-width: 34rem; padding: 2rem; }
      h1 { font-size: 1.25rem; margin: 0 0 .75rem; letter-spacing: .01em; }
      p { color: #7d93a6; margin: 0 0 1rem; }
      code { display: block; padding: .75rem 1rem; background: #0c1218;
             border: 1px solid #1e2c38; border-radius: 2px; color: #fff; }
      a { color: #3ddbff; }
    </style>
  </head>
  <body>
    <div>
      <h1>The client was not built into this binary</h1>
      <p>
        The API is running normally — try
        <a href="/api/v1/aircraft">/api/v1/aircraft</a> or
        <a href="/healthz">/healthz</a>. Only the web interface is missing.
      </p>
      <p>Build it, then rebuild the binary:</p>
      <code>cd client &amp;&amp; npm install &amp;&amp; npm run build<br />cargo build --release -p adsb-server</code>
    </div>
  </body>
</html>
"#;

    if let Err(e) = std::fs::write(client.join("index.html"), placeholder) {
        println!("cargo:warning=could not write client placeholder: {e}");
    }
}
