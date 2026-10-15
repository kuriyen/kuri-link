//! Back ends linked into the same process as their front end.
//!
//! An app that can run its back end on the device registers it here at start-up;
//! the front end then picks [`Client::local`](crate::Client::local) or a remote
//! connection by configuration, and the rest of its code is the same.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::service::Service;

type Registry = RwLock<HashMap<String, Arc<dyn Service>>>;

fn registry() -> &'static Registry {
    static LOCAL: OnceLock<Registry> = OnceLock::new();
    LOCAL.get_or_init(Default::default)
}

/// Makes `service` available in-process as application `app`. Registering the
/// same name again replaces the earlier service.
pub fn register_local(app: &str, service: impl Service) {
    registry()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(app.to_owned(), Arc::new(service));
}

pub(crate) fn lookup(app: &str) -> Option<Arc<dyn Service>> {
    registry()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(app)
        .cloned()
}
