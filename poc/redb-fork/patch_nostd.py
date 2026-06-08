#!/usr/bin/env python3
"""Applique les remplacements no_std sur tous les fichiers .rs du fork redb."""

import os, re, sys

SRC = os.path.join(os.path.dirname(__file__), "src")

SIMPLE = [
    # std::sync → spin/alloc
    ("use std::sync::Arc;", "use alloc::sync::Arc;"),
    ("use std::sync::Mutex;", "use spin::Mutex;"),
    ("use std::sync::{Arc, Mutex};", "use alloc::sync::Arc;\nuse spin::Mutex;"),
    ("use std::sync::{Arc, Mutex, MutexGuard};", "use alloc::sync::Arc;\nuse spin::{Mutex, MutexGuard};"),
    ("use std::sync::{Arc, Mutex, MutexGuard, RwLock};", "use alloc::sync::Arc;\nuse spin::{Mutex, MutexGuard, RwLock};"),
    ("use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};", "use spin::{RwLock, RwLockReadGuard, RwLockWriteGuard};"),
    ("use std::sync::{Condvar, Mutex};", "use crate::compat::sync::{Condvar, Mutex};"),
    ("use std::sync::PoisonError;", "use crate::compat::sync::PoisonError;"),
    ("use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};", "use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};"),
    ("use std::sync::atomic::{AtomicBool, Ordering};", "use core::sync::atomic::{AtomicBool, Ordering};"),
    ("use std::sync::atomic::AtomicU64;", "use core::sync::atomic::AtomicU64;"),
    # collections
    ("use std::collections::HashMap;", "use hashbrown::HashMap;"),
    ("use std::collections::HashSet;", "use hashbrown::HashSet;"),
    ("use std::collections::{HashMap, HashSet};", "use hashbrown::{HashMap, HashSet};"),
    ("use std::collections::{BTreeMap, HashMap, HashSet};", "use alloc::collections::BTreeMap;\nuse hashbrown::{HashMap, HashSet};"),
    ("use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};", "use alloc::collections::{BTreeMap, BTreeSet};\nuse hashbrown::{HashMap, HashSet};"),
    ("use std::collections::{BTreeMap, BTreeSet};", "use alloc::collections::{BTreeMap, BTreeSet};"),
    ("use std::collections::{BTreeMap, HashMap};", "use alloc::collections::BTreeMap;\nuse hashbrown::HashMap;"),
    ("use std::collections::{BTreeSet, HashMap};", "use alloc::collections::BTreeSet;\nuse hashbrown::HashMap;"),
    ("use std::collections::BTreeMap;", "use alloc::collections::BTreeMap;"),
    ("use std::collections::BTreeSet;", "use alloc::collections::BTreeSet;"),
    ("use std::collections::VecDeque;", "use alloc::collections::VecDeque;"),
    ("use std::collections::btree_map::BTreeMap;", "use alloc::collections::btree_map::BTreeMap;"),
    ("use std::collections::Bound;", "use core::ops::Bound;"),
    # core types
    ("use std::fmt::{Debug, Display, Formatter};", "use core::fmt::{Debug, Display, Formatter};"),
    ("use std::fmt::{Debug, Formatter};", "use core::fmt::{Debug, Formatter};"),
    ("use std::fmt::{Display, Formatter};", "use core::fmt::{Display, Formatter};"),
    ("use std::fmt::Debug;", "use core::fmt::Debug;"),
    ("use std::fmt;", "use core::fmt;"),
    ("use std::hash::{Hash, Hasher};", "use core::hash::{Hash, Hasher};"),
    ("use std::hash::{BuildHasherDefault, Hasher};", "use core::hash::{BuildHasherDefault, Hasher};"),
    ("use std::hash::Hasher;", "use core::hash::Hasher;"),
    ("use std::marker::PhantomData;", "use core::marker::PhantomData;"),
    ("use std::mem;", "use core::mem;"),
    ("use std::ops::{RangeBounds, RangeFull};", "use core::ops::{RangeBounds, RangeFull};"),
    ("use std::ops::RangeBounds;", "use core::ops::RangeBounds;"),
    ("use std::cmp::{max, min};", "use core::cmp::{max, min};"),
    ("use std::cmp::{self, max};", "use core::cmp::{self, max};"),
    ("use std::cmp::max;", "use core::cmp::max;"),
    ("use std::cmp::min;", "use core::cmp::min;"),
    ("use std::cmp::Ordering;", "use core::cmp::Ordering;"),
    ("use std::cmp;", "use core::cmp;"),
    ("use std::convert::TryInto;", "use core::convert::TryInto;"),
    ("use std::slice::SliceIndex;", "use core::slice::SliceIndex;"),
    ("use std::borrow::Borrow;", "use alloc::borrow::Borrow;"),
    ("use std::vec::Vec;", "use alloc::vec::Vec;"),
    ("use std::string::String;", "use alloc::string::String;"),
    ("use std::boxed::Box;", "use alloc::boxed::Box;"),
    # io
    ("use std::io::Error;", "use crate::compat::io::Error;"),
    ("use std::io::ErrorKind;", "use crate::compat::io::ErrorKind;"),
    ("use std::io;", "use crate::compat::io;"),
    # thread
    ("thread::panicking()", "false"),
    # inline path refs
    ("std::sync::atomic::", "core::sync::atomic::"),
    ("std::sync::Arc", "alloc::sync::Arc"),
    ("std::cmp::", "core::cmp::"),
    ("std::mem::", "core::mem::"),
    ("std::ptr::", "core::ptr::"),
    ("std::str::", "core::str::"),
    ("std::iter::", "core::iter::"),
    ("std::fmt::", "core::fmt::"),
    ("std::marker::", "core::marker::"),
    ("std::ops::", "core::ops::"),
    ("std::hash::", "core::hash::"),
    ("std::slice::", "core::slice::"),
    ("std::collections::Bound", "core::ops::Bound"),
    ("std::crate::compat::io::Error", "io::Error"),
    ("crate::compat::io::Error", "io::Error"),
    ("crate::compat::io::ErrorKind", "io::ErrorKind"),
]

# lock unwrap additions (added)
SIMPLE += [
    # spin::Mutex n'a pas de LockResult — supprimer .unwrap() après .lock()/.read()/.write()
    (".lock().unwrap()", ".lock()"),
    (".write().unwrap()", ".write()"),
    (".read().unwrap()", ".read()"),
    (".try_write().map_err(", ".try_write().ok_or("),  # adaptation spin Result→Option
]

REMOVE_LINE = [
    "use std::fs::",
    "use std::path::",
    "use std::os::",
    "use std::thread;",
    "use std::{io, thread};",
    "use std::{mem, thread};",
    "use std::{io, panic};",
    "use std::{panic, thread};",
    "use crate::tree_store::file_backend::",
    "use std::fs::File;",
    "use std::fs::OpenOptions;",
]

REMOVE_IMPL = re.compile(r"^impl std::error::Error for \w+.*\{\}\s*$")


def patch_file(path):
    with open(path) as f:
        content = f.read()
    original = content

    # Apply simple replacements
    for old, new in SIMPLE:
        content = content.replace(old, new)

    # Remove whole lines
    lines = content.splitlines(keepends=True)
    new_lines = []
    for line in lines:
        skip = any(tok in line for tok in REMOVE_LINE)
        if skip:
            continue
        if REMOVE_IMPL.match(line.strip()):
            continue
        new_lines.append(line)
    content = "".join(new_lines)

    if content != original:
        with open(path, "w") as f:
            f.write(content)
        return True
    return False


changed = 0
for root, dirs, files in os.walk(SRC):
    for fn in files:
        if not fn.endswith(".rs"):
            continue
        if fn in ("compat.rs", "lib.rs"):
            continue
        p = os.path.join(root, fn)
        if patch_file(p):
            changed += 1
            print(f"  patched: {p[len(SRC)+1:]}")

print(f"\n{changed} files patched")
