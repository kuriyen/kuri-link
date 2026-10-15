//! The peer set on its own: clones that share it, and the file behind it.

use std::path::PathBuf;

use kuri_link::{PeerSet, SecretKey};

/// A fresh directory under the system's temporary one, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let name = format!("kuri-link-{}", SecretKey::generate().public().fmt_short());
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_peer_set_on_a_file_writes_every_change_through() {
    let dir = Scratch::new();
    let file = dir.0.join("peers");
    let set = PeerSet::open(&file).unwrap();
    assert!(set.is_empty());
    assert!(!file.exists(), "nothing is written until something changes");

    let a = SecretKey::generate().public();
    let b = SecretKey::generate().public();
    let shared = set.clone();
    assert!(set.insert(a).unwrap());
    assert!(!set.insert(a).unwrap(), "already there");
    assert!(shared.insert(b).unwrap());
    assert!(
        set.contains(&b) && shared.contains(&a),
        "clones share one set"
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.lines().count(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    assert!(set.remove(&a).unwrap());
    assert!(!set.remove(&a).unwrap());
    assert_eq!(PeerSet::open(&file).unwrap().peers(), vec![b]);

    // Edited by hand: comments and blank lines are skipped; anything else
    // that is not a key is an error.
    std::fs::write(&file, format!("# the desktop\n\n  {a}  \n")).unwrap();
    assert_eq!(PeerSet::open(&file).unwrap().peers(), vec![a]);
    std::fs::write(&file, "not a key\n").unwrap();
    let e = PeerSet::open(&file).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
}
