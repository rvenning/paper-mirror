//! Generates THIRD-PARTY-NOTICES.txt for a release: every crate linked into the
//! shipped executables (normal dependencies for the Windows target; not dev, build or
//! proc-macro dependencies), with its licence files, plus PDFium and its bundled libraries.
//! Identical licence texts are printed once with the list of crates they cover.
//!
//!     cargo run --example notices -- dist/THIRD-PARTY-NOTICES.txt

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::Value;

const TARGET: &str = "x86_64-pc-windows-gnullvm";

struct Pkg {
    name: String,
    version: String,
    license: String,
    repository: String,
    dir: PathBuf,
}

fn licence_files(p: &Pkg) -> Vec<(String, String)> {
    // Slint is used under its Royalty-free licence 2.0 (see the About window).
    if p.license.contains("LicenseRef-Slint-Royalty-free-2.0") {
        let f = p.dir.join("LICENSES").join("LicenseRef-Slint-Royalty-free-2.0.md");
        if let Ok(t) = std::fs::read_to_string(&f) {
            return vec![("LicenseRef-Slint-Royalty-free-2.0.md".into(), t)];
        }
    }
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&p.dir) else { return out };
    let mut names: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    names.sort();
    for path in names {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let upper = name.to_ascii_uppercase();
        let is_licence =
            ["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE", "COPYRIGHT"].iter().any(|p| upper.starts_with(p));
        if is_licence
            && path.is_file()
            && let Ok(t) = std::fs::read_to_string(&path)
        {
            out.push((name, t));
        }
    }
    out
}

fn normalise(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "THIRD-PARTY-NOTICES.txt".into()));
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let meta = std::process::Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--filter-platform", TARGET])
        .output()
        .expect("cargo metadata");
    assert!(meta.status.success(), "{}", String::from_utf8_lossy(&meta.stderr));
    let v: Value = serde_json::from_slice(&meta.stdout).unwrap();

    let mut pkgs: HashMap<String, Pkg> = HashMap::new();
    for p in v["packages"].as_array().unwrap() {
        let manifest = PathBuf::from(p["manifest_path"].as_str().unwrap());
        pkgs.insert(
            p["id"].as_str().unwrap().to_string(),
            Pkg {
                name: p["name"].as_str().unwrap().into(),
                version: p["version"].as_str().unwrap().into(),
                license: p["license"].as_str().unwrap_or("see licence files").into(),
                repository: p["repository"].as_str().unwrap_or("").into(),
                dir: manifest.parent().unwrap().to_path_buf(),
            },
        );
    }
    let resolve = &v["resolve"];
    let root = resolve["root"].as_str().unwrap().to_string();
    let nodes: HashMap<String, &Value> =
        resolve["nodes"].as_array().unwrap().iter().map(|n| (n["id"].as_str().unwrap().to_string(), n)).collect();
    // Walk normal dependencies only: those are what the executables contain.
    let mut seen = BTreeSet::new();
    let mut stack = vec![root.clone()];
    while let Some(id) = stack.pop() {
        let Some(node) = nodes.get(&id) else { continue };
        for dep in node["deps"].as_array().unwrap() {
            let normal = dep["dep_kinds"].as_array().unwrap().iter().any(|k| k["kind"].is_null());
            let pkg = dep["pkg"].as_str().unwrap().to_string();
            if normal && seen.insert(pkg.clone()) {
                stack.push(pkg);
            }
        }
    }
    seen.remove(&root);
    // cargo metadata unifies features with dev-dependencies (the test backend pulls
    // in renderers Paper Mirror doesn't ship) and lists proc-macros, which run at
    // build time only. cargo tree without those edges gives exactly what is linked.
    let tree = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["tree", "-e", "normal,no-proc-macro", "--target", TARGET, "--prefix", "none", "-f", "{p}"])
        .output()
        .expect("cargo tree");
    assert!(tree.status.success(), "{}", String::from_utf8_lossy(&tree.stderr));
    let linked: BTreeSet<(String, String)> = String::from_utf8_lossy(&tree.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.to_string(), it.next()?.trim_start_matches('v').to_string()))
        })
        .collect();
    seen.retain(|id| linked.contains(&(pkgs[id].name.clone(), pkgs[id].version.clone())));

    // Group crates by identical licence text.
    let mut groups: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    let mut without: Vec<String> = Vec::new();
    let mut listing: Vec<String> = Vec::new();
    let mut ids: Vec<&String> = seen.iter().collect();
    ids.sort_by_key(|id| (pkgs[*id].name.to_lowercase(), pkgs[*id].version.clone()));
    for id in ids {
        let p = &pkgs[id];
        let line = format!(
            "{} {} — {}{}",
            p.name,
            p.version,
            p.license,
            if p.repository.is_empty() { String::new() } else { format!(" — {}", p.repository) }
        );
        listing.push(line.clone());
        let files = licence_files(p);
        if files.is_empty() {
            without.push(line);
        }
        for (_, text) in files {
            groups
                .entry(normalise(&text))
                .or_insert_with(|| (text.clone(), Vec::new()))
                .1
                .push(format!("{} {}", p.name, p.version));
        }
    }

    let mut s = String::new();
    s.push_str("Paper Mirror — third-party notices\n");
    s.push_str("===================================\n\n");
    s.push_str("Paper Mirror includes the software listed below. Each component is used under\n");
    s.push_str("the licence shown; the licence texts follow. Slint is used under the Slint\n");
    s.push_str("Royalty-free Desktop, Mobile, and Web Applications License 2.0.\n\n");
    s.push_str(&format!("Rust crates ({}):\n\n", listing.len()));
    for l in &listing {
        s.push_str(&format!("  {l}\n"));
    }
    s.push('\n');
    if !without.is_empty() {
        s.push_str("These crates ship no licence file; their licence is the SPDX expression shown:\n\n");
        for l in &without {
            s.push_str(&format!("  {l}\n"));
        }
        s.push_str("\nTheir copyright belongs to the authors named in each repository. The Apache-2.0\n");
        s.push_str("text appears further below; the MIT, BSD-3-Clause and BSL-1.0 texts are:\n\n");
        s.push_str(STANDARD_TEXTS);
        s.push('\n');
    }

    let pdfium = Path::new("third_party/pdfium");
    s.push_str(&"=".repeat(79));
    s.push_str("\nPDFium (chromium/");
    s.push_str(
        std::fs::read_to_string(pdfium.join("VERSION"))
            .unwrap_or_default()
            .lines()
            .find_map(|l| l.strip_prefix("BUILD="))
            .unwrap_or("?"),
    );
    s.push_str("), built by bblanchon/pdfium-binaries\n");
    s.push_str(&"=".repeat(79));
    s.push_str("\n\n");
    if let Ok(t) = std::fs::read_to_string(pdfium.join("LICENSE")) {
        s.push_str(&t);
        s.push_str("\n\n");
    }
    if let Ok(rd) = std::fs::read_dir(pdfium.join("licenses")) {
        let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.sort();
        for f in files {
            s.push_str(&format!("--- {} ---\n\n", f.file_name().unwrap().to_string_lossy()));
            s.push_str(&std::fs::read_to_string(&f).unwrap_or_default());
            s.push_str("\n\n");
        }
    }

    for (text, crates) in groups.values() {
        s.push_str(&"=".repeat(79));
        s.push('\n');
        for chunk in crates.chunks(6) {
            s.push_str(&chunk.join(", "));
            s.push('\n');
        }
        s.push_str(&"=".repeat(79));
        s.push_str("\n\n");
        s.push_str(text.trim_end());
        s.push_str("\n\n");
    }
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&out, s.replace("\r\n", "\n").replace('\n', "\r\n")).unwrap();
    println!("wrote {} ({} crates, {} distinct licence texts)", out.display(), listing.len(), groups.len());
}

const STANDARD_TEXTS: &str = "--- MIT ---

Permission is hereby granted, free of charge, to any person obtaining a copy of this
software and associated documentation files (the \"Software\"), to deal in the Software
without restriction, including without limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.

--- BSD-3-Clause ---

Redistribution and use in source and binary forms, with or without modification, are
permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of
   conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice, this list of
   conditions and the following disclaimer in the documentation and/or other materials
   provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors may be used
   to endorse or promote products derived from this software without specific prior
   written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS \"AS IS\" AND ANY
EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR
TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

--- BSL-1.0 (Boost Software License 1.0) ---

Permission is hereby granted, free of charge, to any person or organization obtaining a
copy of the software and accompanying documentation covered by this license (the
\"Software\") to use, reproduce, display, distribute, execute, and transmit the Software,
and to prepare derivative works of the Software, and to permit third-parties to whom the
Software is furnished to do so, all subject to the following:

The copyright notices in the Software and this entire statement, including the above
license grant, this restriction and the following disclaimer, must be included in all
copies of the Software, in whole or in part, and all derivative works of the Software,
unless such copies or derivative works are solely in the form of machine-executable
object code generated by a source language processor.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
PURPOSE, TITLE AND NON-INFRINGEMENT. IN NO EVENT SHALL THE COPYRIGHT HOLDERS OR ANYONE
DISTRIBUTING THE SOFTWARE BE LIABLE FOR ANY DAMAGES OR OTHER LIABILITY, WHETHER IN
CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR
THE USE OR OTHER DEALINGS IN THE SOFTWARE.
";
