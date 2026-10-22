//! A serialized I/O queue keeps recovery writes and saves in document order.
use gtk::glib;
use std::sync::{OnceLock, mpsc};
type Job = Box<dyn FnOnce() + Send>;
static IO: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
pub fn run<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let queue = IO.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("notes-io".into())
            .spawn(move || {
                for job in receiver {
                    job();
                }
            })
            .expect("start notes I/O worker");
        sender
    });
    let (sender, receiver) = async_channel::bounded(1);
    queue
        .send(Box::new(move || {
            let _ = sender.send_blocking(work());
        }))
        .expect("notes I/O worker stopped");
    glib::spawn_future_local(async move {
        if let Ok(value) = receiver.recv().await {
            done(value);
        }
    });
}
/// Index rebuilds have a separate worker so a large vault cannot delay a save.
pub fn background<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let (sender, receiver) = async_channel::bounded(1);
    static INDEX: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    let queue = INDEX.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("notes-index".into())
            .spawn(move || {
                for job in receiver {
                    job();
                }
            })
            .expect("start notes index worker");
        sender
    });
    queue
        .send(Box::new(move || {
            let _ = sender.send_blocking(work());
        }))
        .expect("notes index worker stopped");
    glib::spawn_future_local(async move {
        if let Ok(value) = receiver.recv().await {
            done(value);
        }
    });
}
/// Optional model work has its own queue; it never delays saves or keyword indexing.
#[cfg(feature = "ai")]
pub fn intelligence<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    static AI: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    let queue = AI.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("notes-ai".into())
            .spawn(move || {
                for job in receiver {
                    job();
                }
            })
            .expect("start model worker");
        sender
    });
    let (sender, receiver) = async_channel::bounded(1);
    queue
        .send(Box::new(move || {
            let _ = sender.send_blocking(work());
        }))
        .expect("model worker stopped");
    glib::spawn_future_local(async move {
        if let Ok(value) = receiver.recv().await {
            done(value);
        }
    });
}

/// WebDAV work never delays local saves, indexing, or models.
#[cfg(feature = "sync")]
pub fn synchronization<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    static SYNC: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    let queue = SYNC.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("notes-sync".into())
            .spawn(move || {
                for job in receiver {
                    job();
                }
            })
            .expect("start sync worker");
        sender
    });
    let (sender, receiver) = async_channel::bounded(1);
    queue
        .send(Box::new(move || {
            let _ = sender.send_blocking(work());
        }))
        .expect("sync worker stopped");
    glib::spawn_future_local(async move {
        if let Ok(value) = receiver.recv().await {
            done(value);
        }
    });
}

/// Third-party processors run away from GTK and local save queues.
#[cfg(feature = "plugins")]
pub fn extension<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    static PLUGINS: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    let queue = PLUGINS.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("notes-extensions".into())
            .spawn(move || {
                for job in receiver {
                    job();
                }
            })
            .expect("start extension worker");
        sender
    });
    let (sender, receiver) = async_channel::bounded(1);
    queue
        .send(Box::new(move || {
            let _ = sender.send_blocking(work());
        }))
        .expect("extension worker stopped");
    glib::spawn_future_local(async move {
        if let Ok(value) = receiver.recv().await {
            done(value);
        }
    });
}
