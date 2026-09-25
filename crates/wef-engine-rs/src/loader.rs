//! Jailed module loader: imports cannot leave the package root.
//!
//! Boa's `SimpleModuleLoader` resolves `..` lexically and rejects escapes —
//! but only lexically. A symlink *inside* the package pointing outside
//! (`link.js -> /etc/secret.js`) normalizes to an in-root path and is then
//! read straight off disk, so a malicious package could import (and, via
//! `ctx.http`, exfiltrate) any JavaScript-readable file on the host.
//!
//! This wrapper keeps Boa's resolver for specifier handling and adds the
//! missing check: after resolution, the path is canonicalized (symlinks
//! resolved — the file must exist since it is read next) and rejected
//! unless it still lands under the package root. Ports: resolve, then
//! `realpath`, then prefix-check — in that order; the order is the fix.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use boa_engine::{
    Context, JsError, JsNativeError, JsResult, JsString, Source,
    module::{Module, ModuleLoader, Referrer, SimpleModuleLoader, resolve_module_specifier},
};

pub(crate) struct JailedModuleLoader {
    inner: SimpleModuleLoader,
    root: PathBuf,
}

impl JailedModuleLoader {
    pub(crate) fn new(root: PathBuf) -> JsResult<Self> {
        // The root itself is canonicalized so the prefix check below cannot
        // be fooled by `root` containing its own symlinks.
        let root = root.canonicalize().map_err(|error| {
            JsNativeError::typ().with_message(format!("could not resolve package root: {error}"))
        })?;
        let inner = SimpleModuleLoader::new(&root).map_err(|error| {
            JsNativeError::typ().with_message(format!(
                "could not configure package module loader: {error}"
            ))
        })?;
        Ok(Self { inner, root })
    }

    /// Caches an already-parsed module (the engine entry point, whose path
    /// `Package` canonicalized at load). Keys are canonical paths, matching
    /// what `load_imported_module` looks up after its own canonicalization.
    pub(crate) fn insert(&self, path: PathBuf, module: Module) {
        self.inner.insert(path, module);
    }
}

impl ModuleLoader for JailedModuleLoader {
    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        specifier: JsString,
        context: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<Module>> {
        let result = (|| {
            let short_path = specifier.to_std_string_escaped();
            let path = resolve_module_specifier(
                Some(&self.root),
                &specifier,
                referrer.path(),
                &mut context.borrow_mut(),
            )?;
            // Canonicalize AFTER resolution: this is where symlinks die.
            // `canonicalize` cannot fail spuriously — the file is read on
            // the very next line, so absence is an error either way.
            let real = path.canonicalize().map_err(|_| {
                JsError::from_opaque(
                    JsString::from(format!("could not open file `{short_path}`")).into(),
                )
            })?;
            if !real.starts_with(&self.root) {
                return Err(JsError::from_opaque(
                    JsString::from(format!("module `{short_path}` escapes the package root"))
                        .into(),
                ));
            }
            if let Some(module) = self.inner.get(&real) {
                return Ok(module);
            }
            let source = Source::from_filepath(&real).map_err(|error| {
                JsNativeError::typ()
                    .with_message(format!("could not open file `{short_path}`"))
                    .with_cause(JsError::from_opaque(
                        JsString::from(error.to_string()).into(),
                    ))
            })?;
            let module =
                Module::parse(source, None, &mut context.borrow_mut()).map_err(|error| {
                    JsNativeError::syntax()
                        .with_message(format!("could not parse module `{short_path}`"))
                        .with_cause(error)
                })?;
            self.inner.insert(real, module.clone());
            Ok(module)
        })();

        async { result }
    }
}
