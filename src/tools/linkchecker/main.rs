// Copyright 2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Script to check the validity of `href` links in our HTML documentation.
//!
//! In the past we've been quite error prone to writing in broken links as most
//! of them are manually rather than automatically added. As files move over
//! time or apis change old links become stale or broken. The purpose of this
//! script is to check all relative links in our documentation to make sure they
//! actually point to a valid place.
//!
//! Currently this doesn't actually do any HTML parsing or anything fancy like
//! that, it just has a simple "regex" to search for `href` and `id` tags.
//! These values are then translated to file URLs if possible and then the
//! destination is asserted to exist.
//!
//! A few whitelisted exceptions are allowed as there's known bugs in rustdoc,
//! but this should catch the majority of "broken link" cases.

use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::path::{Path, PathBuf, Component};
use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;

use Redirect::*;

macro_rules! t {
    ($e:expr) => (match $e {
        Ok(e) => e,
        Err(e) => panic!("{} failed with {:?}", stringify!($e), e),
    })
}

fn main() {
    let docs = env::args_os().nth(1).unwrap();
    let docs = env::current_dir().unwrap().join(docs);
    let mut errors = false;
    walk(&mut HashMap::new(), &docs, &docs, &mut errors);
    if errors {
        panic!("found some broken links");
    }
}

#[derive(Debug)]
pub enum LoadError {
    IOError(std::io::Error),
    BrokenRedirect(PathBuf, std::io::Error),
    IsRedirect,
}

enum Redirect {
    SkipRedirect,
    FromRedirect(bool),
}

struct FileEntry {
    source: String,
    ids: HashSet<String>,
}

type Cache = HashMap<PathBuf, FileEntry>;

fn small_url_encode(s: &str) -> String {
    s.replace("<", "%3C")
     .replace(">", "%3E")
     .replace(" ", "%20")
     .replace("?", "%3F")
     .replace("'", "%27")
     .replace("&", "%26")
     .replace(",", "%2C")
     .replace(":", "%3A")
     .replace(";", "%3B")
     .replace("[", "%5B")
     .replace("]", "%5D")
     .replace("\"", "%22")
}

impl FileEntry {
    fn parse_ids(&mut self, file: &Path, contents: &str, errors: &mut bool) {
        if self.ids.is_empty() {
            with_attrs_in_source(contents, " id", |fragment, i, _| {
                let frag = fragment.trim_left_matches("#").to_owned();
                let encoded = small_url_encode(&frag);
                if !self.ids.insert(frag) {
                    *errors = true;
                    println!("{}:{}: id is not unique: `{}`", file.display(), i, fragment);
                }
                // Just in case, we also add the encoded id.
                self.ids.insert(encoded);
            });
        }
    }
}

fn walk(cache: &mut Cache, root: &Path, dir: &Path, errors: &mut bool) {
    for entry in t!(dir.read_dir()).map(|e| t!(e)) {
        let path = entry.path();
        let kind = t!(entry.file_type());
        if kind.is_dir() {
            walk(cache, root, &path, errors);
        } else {
            let pretty_path = check(cache, root, &path, errors);
            if let Some(pretty_path) = pretty_path {
                let entry = cache.get_mut(&pretty_path).unwrap();
                // we don't need the source anymore,
                // so drop to reduce memory-usage
                entry.source = String::new();
            }
        }
    }
}

fn check(cache: &mut Cache,
         root: &Path,
         file: &Path,
         errors: &mut bool)
         -> Option<PathBuf> {
    // Ignore none HTML files.
    if file.extension().and_then(|s| s.to_str()) != Some("html") {
        return None;
    }

    // Unfortunately we're not 100% full of valid links today to we need a few
    // whitelists to get this past `make check` today.
    // FIXME(#32129)
    if file.ends_with("std/string/struct.String.html") ||
       file.ends_with("interpret/struct.ValTy.html") ||
       file.ends_with("symbol/struct.InternedString.html") ||
       file.ends_with("ast/struct.ThinVec.html") ||
       file.ends_with("util/struct.ThinVec.html") ||
       file.ends_with("util/struct.RcSlice.html") ||
       file.ends_with("layout/struct.TyLayout.html") ||
       file.ends_with("ty/struct.Slice.html") ||
       file.ends_with("ty/enum.Attributes.html") ||
       file.ends_with("ty/struct.SymbolName.html") {
        return None;
    }
    // FIXME(#32553)
    if file.ends_with("string/struct.String.html") {
        return None;
    }
    // FIXME(#32130)
    if file.ends_with("btree_set/struct.BTreeSet.html") ||
       file.ends_with("struct.BTreeSet.html") ||
       file.ends_with("btree_map/struct.BTreeMap.html") ||
       file.ends_with("hash_map/struct.HashMap.html") ||
       file.ends_with("hash_set/struct.HashSet.html") ||
       file.ends_with("sync/struct.Lrc.html") ||
       file.ends_with("sync/struct.RwLock.html") {
        return None;
    }
    // FIXME(#47038)
    if file.ends_with("deriving/generic/index.html") ||
       file.ends_with("deriving/generic/macro.vec.html") ||
       file.ends_with("deriving/custom/macro.panic.html") ||
       file.ends_with("proc_macro_impl/macro.panic.html") {
        return None;
    }

    let res = load_file(cache, root, file, SkipRedirect);
    let (pretty_file, contents) = match res {
        Ok(res) => res,
        Err(_) => return None,
    };
    {
        cache.get_mut(&pretty_file)
             .unwrap()
             .parse_ids(&pretty_file, &contents, errors);
    }

    // Search for anything that's the regex 'href[ ]*=[ ]*".*?"'
    with_attrs_in_source(&contents, " href", |url, i, base| {
        // Ignore external URLs
        if url.starts_with("http:") || url.starts_with("https:") ||
           url.starts_with("javascript:") || url.starts_with("ftp:") ||
           url.starts_with("irc:") || url.starts_with("data:") {
            return;
        }
        let mut parts = url.splitn(2, "#");
        let url = parts.next().unwrap();
        let fragment = parts.next();
        let mut parts = url.splitn(2, "?");
        let url = parts.next().unwrap();

        // Once we've plucked out the URL, parse it using our base url and
        // then try to extract a file path.
        let mut path = file.to_path_buf();
        if !base.is_empty() || !url.is_empty() {
            path.pop();
            for part in Path::new(base).join(url).components() {
                match part {
                    Component::Prefix(_) |
                    Component::RootDir => panic!(),
                    Component::CurDir => {}
                    Component::ParentDir => { path.pop(); }
                    Component::Normal(s) => { path.push(s); }
                }
            }
        }

        // Alright, if we've found a file name then this file had better
        // exist! If it doesn't then we register and print an error.
        if path.exists() {
            if path.is_dir() {
                // Links to directories show as directory listings when viewing
                // the docs offline so it's best to avoid them.
                *errors = true;
                let pretty_path = path.strip_prefix(root).unwrap_or(&path);
                println!("{}:{}: directory link - {}",
                         pretty_file.display(),
                         i + 1,
                         pretty_path.display());
                return;
            }
            if let Some(extension) = path.extension() {
                // Ignore none HTML files.
                if extension != "html" {
                    return;
                }
            }
            let res = load_file(cache, root, &path, FromRedirect(false));
            let (pretty_path, contents) = match res {
                Ok(res) => res,
                Err(LoadError::IOError(err)) => {
                    panic!("error loading {}: {}", path.display(), err);
                }
                Err(LoadError::BrokenRedirect(target, _)) => {
                    *errors = true;
                    println!("{}:{}: broken redirect to {}",
                             pretty_file.display(),
                             i + 1,
                             target.display());
                    return;
                }
                Err(LoadError::IsRedirect) => unreachable!(),
            };

            if let Some(ref fragment) = fragment {
                // Fragments like `#1-6` are most likely line numbers to be
                // interpreted by javascript, so we're ignoring these
                if fragment.splitn(2, '-')
                           .all(|f| f.chars().all(|c| c.is_numeric())) {
                    return;
                }

                let entry = &mut cache.get_mut(&pretty_path).unwrap();
                entry.parse_ids(&pretty_path, &contents, errors);

                if !entry.ids.contains(*fragment) {
                    *errors = true;
                    print!("{}:{}: broken link fragment ",
                           pretty_file.display(),
                           i + 1);
                    println!("`#{}` pointing to `{}`", fragment, pretty_path.display());
                };
            }
        } else {
            *errors = true;
            print!("{}:{}: broken link - ", pretty_file.display(), i + 1);
            let pretty_path = path.strip_prefix(root).unwrap_or(&path);
            println!("{}", pretty_path.display());
        }
    });
    Some(pretty_file)
}

fn load_file(cache: &mut Cache,
             root: &Path,
             file: &Path,
             redirect: Redirect)
             -> Result<(PathBuf, String), LoadError> {
    let mut contents = String::new();
    let pretty_file = PathBuf::from(file.strip_prefix(root).unwrap_or(&file));

    let maybe_redirect = match cache.entry(pretty_file.clone()) {
        Entry::Occupied(entry) => {
            contents = entry.get().source.clone();
            None
        }
        Entry::Vacant(entry) => {
            let mut fp = File::open(file).map_err(|err| {
                if let FromRedirect(true) = redirect {
                    LoadError::BrokenRedirect(file.to_path_buf(), err)
                } else {
                    LoadError::IOError(err)
                }
            })?;
            fp.read_to_string(&mut contents).map_err(|err| LoadError::IOError(err))?;

            let maybe = maybe_redirect(&contents);
            if maybe.is_some() {
                if let SkipRedirect = redirect {
                    return Err(LoadError::IsRedirect);
                }
            } else {
                entry.insert(FileEntry {
                    source: contents.clone(),
                    ids: HashSet::new(),
                });
            }
            maybe
        }
    };
    match maybe_redirect.map(|url| file.parent().unwrap().join(url)) {
        Some(redirect_file) => {
            load_file(cache, root, &redirect_file, FromRedirect(true))
        }
        None => Ok((pretty_file, contents)),
    }
}

fn maybe_redirect(source: &str) -> Option<String> {
    const REDIRECT: &'static str = "<p>Redirecting to <a href=";

    let mut lines = source.lines();
    let redirect_line = match lines.nth(6) {
        Some(l) => l,
        None => return None,
    };

    redirect_line.find(REDIRECT).map(|i| {
        let rest = &redirect_line[(i + REDIRECT.len() + 1)..];
        let pos_quote = rest.find('"').unwrap();
        rest[..pos_quote].to_owned()
    })
}

fn with_attrs_in_source<F: FnMut(&str, usize, &str)>(contents: &str, attr: &str, mut f: F) {
    let mut base = "";
    for (i, mut line) in contents.lines().enumerate() {
        while let Some(j) = line.find(attr) {
            let rest = &line[j + attr.len()..];
            // The base tag should always be the first link in the document so
            // we can get away with using one pass.
            let is_base = line[..j].ends_with("<base");
            line = rest;
            let pos_equals = match rest.find("=") {
                Some(i) => i,
                None => continue,
            };
            if rest[..pos_equals].trim_left_matches(" ") != "" {
                continue;
            }

            let rest = &rest[pos_equals + 1..];

            let pos_quote = match rest.find(&['"', '\''][..]) {
                Some(i) => i,
                None => continue,
            };
            let quote_delim = rest.as_bytes()[pos_quote] as char;

            if rest[..pos_quote].trim_left_matches(" ") != "" {
                continue;
            }
            let rest = &rest[pos_quote + 1..];
            let url = match rest.find(quote_delim) {
                Some(i) => &rest[..i],
                None => continue,
            };
            if is_base {
                base = url;
                continue;
            }
            f(url, i, base)
        }
    }
}
