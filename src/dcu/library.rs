//! A compiler's unit directory: every DCU of one release and platform,
//! read on demand, with type resolution across units.
//!
//! A unit refers to another unit's types by import (unit name and symbol
//! name). Following such a reference to the definition - to learn that
//! `System.Classes.TComponent` is a class or that `Winapi.Windows.UINT` is
//! `System.Cardinal` - needs the other unit's DCU; [`Library`] indexes a
//! directory of `.dcu` and `.dcp` files and resolves references through it.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::dcu::{DeclKind, TypeTarget, Unit, package::Package};

/// Where a unit's DCU image lies.
#[derive(Debug, Clone)]
struct Source {
    path: PathBuf,
    offset: usize,
    len: Option<usize>,
    from_package: bool,
}

/// The DCUs of one compiler release and platform.
#[derive(Debug, Default)]
pub struct Library {
    index: HashMap<String, Source>,
    cache: Mutex<HashMap<String, Option<Arc<Unit>>>>,
}

/// A type definition found by [`Library::resolve`]: the unit that defines
/// it and its type-table index there.
#[derive(Debug, Clone)]
pub struct ResolvedType {
    /// The defining unit.
    pub unit: Arc<Unit>,
    /// The type-table index in that unit.
    pub index: u32,
}

impl ResolvedType {
    /// Returns the type's declared name.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.unit.type_slot(self.index)?.name.as_deref()
    }
}

impl Library {
    /// Indexes every `.dcu` in `dir` and every unit inside every `.dcp`
    /// there. A unit present both standalone and in a package is read from
    /// the standalone file.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the directory cannot be listed or a package
    /// read.
    pub fn open(dir: &Path) -> io::Result<Self> {
        Self::open_dirs(&[dir])
    }

    /// Indexes several directories, as [`Library::open`] does one; a unit
    /// found in more than one is read from the first directory that has
    /// it (a project's own units before the compiler's library).
    ///
    /// # Errors
    ///
    /// Returns an I/O error if a directory cannot be listed or a package
    /// read.
    pub fn open_dirs(dirs: &[&Path]) -> io::Result<Self> {
        let mut index = HashMap::new();
        for dir in dirs.iter().rev() {
            let part = Self::index_dir(dir)?;
            index.extend(part);
        }
        Ok(Self {
            index,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Indexes one directory.
    fn index_dir(dir: &Path) -> io::Result<HashMap<String, Source>> {
        let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        paths.sort();
        let ext = |p: &Path| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
        };
        let mut index = HashMap::new();
        for path in paths.iter().filter(|p| ext(p).as_deref() == Some("dcp")) {
            let data = fs::read(path)?;
            let Ok(package) = Package::parse(&data, false) else {
                continue;
            };
            for unit in package.units {
                index.insert(
                    unit.name.to_ascii_lowercase(),
                    Source {
                        path: path.clone(),
                        offset: unit.offset,
                        len: Some(unit.size),
                        from_package: true,
                    },
                );
            }
        }
        for path in paths.iter().filter(|p| ext(p).as_deref() == Some("dcu")) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                index.insert(
                    stem.to_ascii_lowercase(),
                    Source {
                        path: path.clone(),
                        offset: 0,
                        len: None,
                        from_package: false,
                    },
                );
            }
        }
        Ok(index)
    }

    /// Returns the number of units the library knows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Returns whether the library knows no unit.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Returns the names (lowercased) of every unit the library knows.
    pub fn unit_names(&self) -> impl Iterator<Item = &str> {
        self.index.keys().map(String::as_str)
    }

    /// Returns the decoded unit `name` (compared without regard to ASCII
    /// case), or `None` when the library lacks it or it does not decode.
    #[must_use]
    pub fn unit(&self, name: &str) -> Option<Arc<Unit>> {
        let key = name.to_ascii_lowercase();
        if let Ok(cache) = self.cache.lock()
            && let Some(hit) = cache.get(&key)
        {
            return hit.clone();
        }
        let unit = self.index.get(&key).and_then(|src| {
            let data = fs::read(&src.path).ok()?;
            let end = match src.len {
                Some(len) => src.offset.checked_add(len)?,
                None => data.len(),
            };
            let bytes = data.get(src.offset..end)?;
            let parsed = if src.from_package {
                Unit::parse_from_package(bytes)
            } else {
                Unit::parse(bytes)
            };
            parsed.ok().map(Arc::new)
        });
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key, unit.clone());
        }
        unit
    }

    /// Follows type `index` of `unit` to its definition: through imports
    /// (by the exporting unit's type declaration of that name) to the unit
    /// whose type table holds the definition. Returns `None` when a unit on
    /// the way is missing from the library.
    #[must_use]
    pub fn resolve(&self, unit: &Arc<Unit>, index: u32) -> Option<ResolvedType> {
        let mut unit = Arc::clone(unit);
        let mut index = index;
        // Bounded: a chain of re-exports is short, and a malformed library
        // must not loop.
        for _ in 0..32 {
            let slot = unit.type_slot(index)?;
            match slot.target {
                TypeTarget::Def(_) | TypeTarget::None => return Some(ResolvedType { unit, index }),
                TypeTarget::Import { unit: u, symbol } => {
                    let import = unit.uses.get(u)?;
                    let name = import.symbols.get(symbol)?.name.clone();
                    let next = self.unit(&import.name)?;
                    let found = next.decls_of(&next.main).find_map(|d| match d.kind {
                        DeclKind::Type { type_index } if d.name.eq_ignore_ascii_case(&name) => {
                            Some(type_index)
                        }
                        _ => None,
                    })?;
                    unit = next;
                    index = found;
                }
            }
        }
        None
    }
}
