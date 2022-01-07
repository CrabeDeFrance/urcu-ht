//! A ReadCopyUpdate (RCU) HashMap implementation. It is build on top of [liburcu].
//! Userspace RCU is a data synchronization library providing read-side access which scales linearly with the number of cores.
//! urcu-ht aims to provide a safe wrapper of liburcu.
//!
//! The default hashing algorithm is currently [wyhash].
//! There is currently no work done to protected it against HashDos.
//!
//! Thanks to this implementation, there is no rwlock or mutex in reader threads.
//! For writer thread, we still need a lock to protect against concurrent insert or remove.
//!
//! [liburcu]: http://liburcu.org/
//! [wyhash]: https://docs.rs/wyhash/0.5.0/wyhash/
//!
//! # Examples
//!
//! ```
//! use urcu_ht::RcuHt;
//!
//! // Type inference lets us omit an explicit type signature (which
//! // would be `HashMap<String, String>` in this example).
//! let ht = RcuHt::new(64, 64, 64, false).expect("Cannot create hashtable, probably due to invalid parameters");
//! let ht = std::sync::Arc::new(ht);
//! // Create a new thread to get book reviews.
//! let child = {
//!     let ht = ht.clone();
//!     std::thread::spawn(move || {
//!         // Get a read handle for this thread
//!         let ht = ht.thread();
//!         // wait until main thread adds some reviews
//!         std::thread::sleep(std::time::Duration::from_millis(100));
//!         
//!         let read = ht.rdlock();
//!         let review = read.get("Adventures of Huckleberry Finn");
//!         match review {
//!             Some(review) => println!("{}: {}", "Adventures of Huckleberry Finn", review),
//!             None => println!("{} is unreviewed.", "Adventures of Huckleberry Finn")
//!         }
//!
//!         // read lock is released when leaving this block
//!     })
//! };
//!
//! let ht = ht.thread();
//! let mut write = ht.wrlock().unwrap();
//! write.insert_or_replace("Adventures of Huckleberry Finn".to_string(),
//!     "My favorite book.".to_string());
//! write.insert_or_replace("Grimms' Fairy Tales".to_string(),
//!     "Masterpiece.".to_string());
//!
//! child.join().expect("cannot join thread");
//! ```
use std::borrow::Borrow;
use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Once;
use std::sync::{Mutex, MutexGuard};

/// Possible error types returned by this module
#[derive(Debug)]
pub enum RcuError {
    /// Impossible to perform the take with provided parameters (allocation of a new hashtable for instance)
    InvalidParameters,
    /// Object is not found in hashtable
    NotFound,
    /// Object cannot be free'd. Hopefully, we do not expect this error to appear.
    /// This probably means we have an implementation error which leads to a memory leak.
    DeleteError(i32),
}

// Rcu object is used only to call once urcu lib initialization (urcu_init)
struct Rcu;

// global flag to know if we need to initialize urcu library (calling urcu_init).
static URCU_LIB_INITIALIZED: Once = Once::new();

impl Rcu {
    pub fn init() {
        URCU_LIB_INITIALIZED.call_once(|| unsafe {
            urcu_sys::rcu_init();
        });
    }
}

/// An RcuHt object is an instance of a RCU hashtable.
pub struct RcuHt<K, V> {
    /// mutex to protect writer (write operation must be done under lock)
    mutex: Mutex<RcuHtWriterGuard<K, V>>,
    /// a pointer to an instance of lib urcu hashtable
    urcuht: *mut urcu_sys::cds_lfht,
}

/// RcuHt can be shared between threads (under std::sync::Arc<>).
unsafe impl<K, V> Send for RcuHt<K, V> {}
/// RcuHt can be shared between threads (under std::sync::Arc<>).
unsafe impl<K, V> Sync for RcuHt<K, V> {}

impl<K, V> RcuHt<K, V>
where
    K: Hash + Eq,
{
    /// Allocate a new instance of urcu hashtable.
    ///
    /// Parameters are mapped to urcu lib : <https://github.com/urcu/userspace-rcu/blob/master/include/urcu/rculfhash.h#L190>
    ///
    /// @init_size: number of buckets to allocate initially. Must be power of two.
    ///
    /// @min_nr_alloc_buckets: the minimum number of allocated buckets. (must be power of two).
    ///
    /// @max_nr_buckets: the maximum number of hash table buckets allowed. (must be power of two, 0 is accepted, means "infinite").
    ///
    /// @autoresize: automatically resize hash table.
    pub fn new(
        init_size: u64,
        min_nr_alloc_buckets: u64,
        max_nr_buckets: u64,
        autoresize: bool,
    ) -> Result<Self, RcuError> {
        // initialize global lib if not already done
        Rcu::init();

        let flags: i32 = match autoresize {
            true => urcu_sys::CDS_LFHT_AUTO_RESIZE as i32,
            false => 0,
        };

        unsafe {
            let urcuht = urcu_sys::cds_lfht_new(
                init_size,
                min_nr_alloc_buckets,
                max_nr_buckets,
                flags,
                std::ptr::null_mut(),
            );

            if urcuht.is_null() {
                return Err(RcuError::InvalidParameters);
            }

            let mutex = Mutex::new(RcuHtWriterGuard::new());

            Ok(RcuHt { urcuht, mutex })
        }
    }

    /// Get a per thread handle. Will be used for read/write operations.
    pub fn thread(&self) -> RcuHtThread<K, V> {
        RcuHtThread::new(self.urcuht, &self.mutex)
    }
}

impl<K, V> Drop for RcuHt<K, V> {
    /// Release an instance of a RCU hashtable.
    fn drop(&mut self) {
        unsafe {
            // must be called when there is no more writer or reader able to access this hashtable.
            // XXX should probably be empty before free ???
            urcu_sys::cds_lfht_destroy(self.urcuht, std::ptr::null_mut());
        }
    }
}

/// This describes every object stored in hashtable.
#[repr(C)]
struct RcuLfhtNode<K, V> {
    /// internal node to link to other objects it in hashtable
    node: urcu_sys::cds_lfht_node,
    /// data structure used for delayed free
    head: urcu_sys::rcu_head,
    /// object key (user data)
    key: K,
    /// object data (user data)
    data: V,
}

/// Match function callback used when looking for objects.
/// Returns 1 if current node key and lookup key are equals, 0 otherwise.
/// Called by urcu_sys::cds_lfht_lookup
/// Unsized callback version
unsafe extern "C" fn urcu_match_ref_fn<Q, K, V>(
    node: *mut urcu_sys::cds_lfht_node,
    key: *const std::ffi::c_void,
) -> i32
where
    K: Borrow<Q>,
    Q: ?Sized + Eq,
{
    let key = *(key as *const &Q);
    let node = urcu_cds_lfht_node_to_rust_type::<K, V>(node);
    let node_key = (*node).key.borrow();

    if key.eq(node_key) {
        1
    } else {
        0
    }
}

/// Match function callback used when looking for objects.
/// Returns 1 if current node key and lookup key are equals, 0 otherwise.
/// Called by urcu_sys::cds_lfht_lookup
/// Sized callback version
unsafe extern "C" fn urcu_match_fn<K, V>(
    node: *mut urcu_sys::cds_lfht_node,
    key: *const std::ffi::c_void,
) -> i32
where
    K: Eq,
{
    let key = key as *const K;
    let node = urcu_cds_lfht_node_to_rust_type::<K, V>(node);

    if (*key).eq(&(*node).key) {
        1
    } else {
        0
    }
}

/// Get back to RcuLfhtNode rust type from a "node" pointer. C-like dark pointer operations and casting.
unsafe fn urcu_cds_lfht_node_to_rust_type<K, V>(
    node: *mut urcu_sys::cds_lfht_node,
) -> *mut RcuLfhtNode<K, V> {
    let offset = memoffset::offset_of!(RcuLfhtNode::<K, V>, node);

    let ptr = node.cast::<u8>();
    ptr.sub(offset).cast::<RcuLfhtNode<K, V>>()
}

/// Get back to RcuLfhtNode rust type from a "head" pointer. C-like dark pointer operations and casting.
unsafe fn urcu_cds_lfht_head_to_rust_type<K, V>(
    node: *mut urcu_sys::rcu_head,
) -> *mut RcuLfhtNode<K, V> {
    let offset = memoffset::offset_of!(RcuLfhtNode::<K, V>, head);

    let ptr = node.cast::<u8>();
    ptr.sub(offset).cast::<RcuLfhtNode<K, V>>()
}

/// Helper function used to perform lookup (used at multiple places).
/// This function must be called with rcu_read_lock held.
/// Threads calling this API need to be registered (urcu_sys::rcu_register_thread).
unsafe fn urcu_get_node<Q, K, V>(
    ht: *mut urcu_sys::cds_lfht,
    key: &Q,
) -> *mut urcu_sys::cds_lfht_node
where
    K: Borrow<Q>,
    Q: ?Sized + Hash + Eq,
{
    let hash = urcu_key_hash(key);

    let mut iter: urcu_sys::cds_lfht_iter = std::mem::MaybeUninit::zeroed().assume_init();

    // cds_lfht_lookup - lookup a node by key.
    // @ht: the hash table.
    // @hash: the key hash.
    // @match: the key match function.
    // @key: the current node key.
    // @iter: node, if found (output). *iter->node set to NULL if not found.
    // This function acts as a rcu_dereference() to read the node pointer.
    urcu_sys::cds_lfht_lookup(
        ht,
        hash,
        Some(urcu_match_ref_fn::<Q, K, V>),
        &key as *const &Q as *const std::ffi::c_void,
        &mut iter as *mut urcu_sys::cds_lfht_iter,
    );

    let found_node: *mut urcu_sys::cds_lfht_node = urcu_sys::cds_lfht_iter_get_node(&mut iter);

    found_node
}

/// helper function to compute a hash of a key.
fn urcu_key_hash<K: ?Sized + Hash>(data: &K) -> u64 {
    let mut hasher = wyhash::WyHash::with_seed(3);
    /*hasher.write(&[0, 1, 2]);*/

    /*let mut hasher = DefaultHasher::new();*/
    data.hash(&mut hasher);
    hasher.finish()
}

/// Callback function, called after some delay, when it is time to free a node.
unsafe extern "C" fn urcu_free_node<K, V>(head: *mut urcu_sys::rcu_head)
where
    K: Hash + Eq,
{
    let node = urcu_cds_lfht_head_to_rust_type::<K, V>(head);

    std::ptr::drop_in_place(&mut (*node).key);
    std::ptr::drop_in_place(&mut (*node).data);

    let layout = std::alloc::Layout::new::<RcuLfhtNode<K, V>>();
    std::alloc::dealloc(node as *mut u8, layout);
}

// thread local flag for thread register / unregister
// since this is a local thread storage, there is no concurrency, so no need for atomics
thread_local! {
    static URCU_THREAD_REGISTERED_COUNT: Cell<u32>  = Cell::new(0);
}

/// Per thread object used to provide safe access to RCU hashtable.
///
/// It registers the current thread if needed (the first reader or writer object triggers the registration).
/// It unregisters the current thread when no more objects are alive in this thread.
pub struct RcuHtThread<'ht, K, V> {
    urcuht: *mut urcu_sys::cds_lfht,
    mutex: &'ht Mutex<RcuHtWriterGuard<K, V>>,
}

impl<'ht, K, V> RcuHtThread<'ht, K, V>
where
    K: Hash + Eq,
{
    /// Get a new "read" handle.
    /// A different handle is needed for each thread doing "read" operations.
    /// It registers this thread in urcu lib.
    /// It must stick to a single thread. One must not try to move this handle between threads.
    pub fn new(urcuht: *mut urcu_sys::cds_lfht, mutex: &'ht Mutex<RcuHtWriterGuard<K, V>>) -> Self {
        // manage thread reference counter : if the count is 1 => register this thread
        let thread_count = URCU_THREAD_REGISTERED_COUNT.with(|cell| {
            let mut thread_count = cell.get();
            thread_count += 1;
            cell.set(thread_count);
            thread_count
        });

        if thread_count == 1 {
            unsafe {
                urcu_sys::rcu_register_thread();
            }
        }

        // return an object with a pointer to the hashtable
        // Return an object with a reference to a shared write mutex for this hashtable.
        RcuHtThread {
            urcuht,
            // This prevent concurrent write on this hashtable.
            // Since mutex is a reference, we are sure original hashtable cannot be deleted before this object.
            // This is needed to protect hashtable deletion.
            mutex,
        }
    }

    pub fn wrlock(&self) -> Option<RcuHtWriter<K, V>> {
        match self.mutex.lock() {
            Ok(guard) => Some(RcuHtWriter::new(self.urcuht, self, guard)),
            Err(_err) => None,
        }
    }

    pub fn rdlock(&self) -> RcuHtRead<K, V> {
        RcuHtRead::new(self.urcuht, self)
    }
}

impl<'ht, K, V> Drop for RcuHtThread<'ht, K, V> {
    fn drop(&mut self) {
        /* manage thread reference counter : if the count is 0 (last object) => unregister this thread */
        let thread_count = URCU_THREAD_REGISTERED_COUNT.with(|cell| {
            let mut thread_count = cell.get();
            thread_count -= 1;
            cell.set(thread_count);
            thread_count
        });

        if thread_count == 0 {
            unsafe {
                urcu_sys::rcu_unregister_thread();
            }
        }
    }
}

pub struct RcuHtRead<'thread, 'ht, K, V> {
    phantom_key: PhantomData<K>,
    phantom_val: PhantomData<V>,
    urcuht: *mut urcu_sys::cds_lfht,
    _thread: &'thread RcuHtThread<'ht, K, V>,
}

impl<'rdlock, 'thread, 'ht, K, V> RcuHtRead<'thread, 'ht, K, V>
where
    K: Hash + Eq,
{
    /// Get a new "read" handle.
    /// A different handle is needed for each thread doing "read" operations.
    /// It registers this thread in urcu lib.
    /// It must stick to a single thread. One must not try to move this handle between threads.
    pub fn new(urcuht: *mut urcu_sys::cds_lfht, thread: &'thread RcuHtThread<'ht, K, V>) -> Self {
        unsafe {
            urcu_sys::rcu_read_lock();
        }

        RcuHtRead {
            phantom_key: PhantomData,
            phantom_val: PhantomData,
            urcuht,
            _thread: thread,
        }
    }

    pub fn get<Q: ?Sized>(&'rdlock self, key: &Q) -> Option<&'rdlock V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
    {
        let mut ret: Option<&V> = None;

        unsafe {
            let found_node = urcu_get_node::<Q, K, V>(self.urcuht, key);

            if !found_node.is_null() {
                let node = urcu_cds_lfht_node_to_rust_type::<K, V>(found_node);
                ret = Some(&(*node).data);
            }
        }

        ret
    }
}

impl<'thread, 'ht, K, V> Drop for RcuHtRead<'thread, 'ht, K, V> {
    fn drop(&mut self) {
        /* manage thread reference counter : if the count is 0 (last object) => unregister this thread */
        unsafe {
            urcu_sys::rcu_read_unlock();
        }
    }
}

pub struct RcuHtWriterGuard<K, V> {
    phantom_key: PhantomData<K>,
    phantom_val: PhantomData<V>,
}

impl<K, V> RcuHtWriterGuard<K, V> {
    fn new() -> Self {
        RcuHtWriterGuard {
            phantom_key: PhantomData,
            phantom_val: PhantomData,
        }
    }
}

/// Writer object used to perform safe add and del operations.
///
/// It can only be called under locked mutex to protect from concurrent access.
/// It must not be shared between threads.
pub struct RcuHtWriter<'guard, 'thread, 'ht, K, V> {
    urcuht: *mut urcu_sys::cds_lfht,
    // keep references to thread so object cannot be destroyed in an invalid order
    _thread: &'thread RcuHtThread<'ht, K, V>,
    // have the guard here so lock will be released when writer is destroyed
    _guard: MutexGuard<'guard, RcuHtWriterGuard<K, V>>,
}

impl<'guard, 'thread, 'ht, K, V> RcuHtWriter<'guard, 'thread, 'ht, K, V>
where
    K: Hash + Eq,
{
    /// Creates a write instance.
    ///
    /// There should be only one single instance allocated under the write mutex.
    fn new(
        urcuht: *mut urcu_sys::cds_lfht,
        thread: &'thread RcuHtThread<'ht, K, V>,
        guard: MutexGuard<'guard, RcuHtWriterGuard<K, V>>,
    ) -> RcuHtWriter<'guard, 'thread, 'ht, K, V> {
        // return an object containing the pointer to the hashtable
        RcuHtWriter {
            urcuht,
            _thread: thread,
            _guard: guard,
        }
    }

    /// Add or replace an existing key/value.
    ///
    /// Parameters (key and value) are moved in hashtable.
    pub fn insert_or_replace(&mut self, key: K, value: V) {
        let h = urcu_key_hash(&key);

        let layout = std::alloc::Layout::new::<RcuLfhtNode<K, V>>();

        unsafe {
            /* allocate a new RcuLfhtNode to store data */
            /* alloc style from https://doc.rust-lang.org/nomicon/vec/vec-alloc.html */

            let ptr = std::alloc::alloc(layout);

            let val = match std::ptr::NonNull::new(ptr as *mut RcuLfhtNode<K, V>) {
                Some(p) => p,
                None => std::alloc::handle_alloc_error(layout),
            };

            // initialize all 4 fields of this new struct
            (*val.as_ptr()).node = std::mem::MaybeUninit::zeroed().assume_init();
            (*val.as_ptr()).head = std::mem::MaybeUninit::zeroed().assume_init();

            let val = &mut *val.as_ptr();

            std::ptr::write(&mut val.key, key);
            std::ptr::write(&mut val.data, value);

            // now add or replace it
            urcu_sys::rcu_read_lock();

            // Return the node replaced upon success. If no node matching the key
            // was present, return NULL, which also means the operation succeeded.
            // This replacement operation should never fail.
            // Call with rcu_read_lock held.
            let old_node: *mut urcu_sys::cds_lfht_node = urcu_sys::cds_lfht_add_replace(
                self.urcuht,
                h,
                Some(urcu_match_fn::<K, V>),
                // coercion allowed from &T to *const T
                // see : https://doc.rust-lang.org/reference/type-coercions.html#coercion-types */
                &val.key as *const K as *const std::ffi::c_void,
                &mut val.node as *mut urcu_sys::cds_lfht_node,
            );

            urcu_sys::rcu_read_unlock();

            // if add_replace returns an node, we must free it
            if !old_node.is_null() {
                // After successful replacement, a grace period must be waited for before
                // freeing or re-using the memory reserved for the returned node.
                let node = urcu_cds_lfht_node_to_rust_type::<K, V>(old_node);

                // ask to free data after grace period
                urcu_sys::urcu_memb_call_rcu(&mut (*node).head, Some(urcu_free_node::<K, V>));
            }
        }
    }

    /// Delete the value indexed by the `key` from the hashtable.
    ///
    /// This function may fail if node is not found.
    pub fn remove<Q: ?Sized>(&mut self, key: &Q) -> Result<(), RcuError>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
    {
        let mut found = false;
        let mut err = 0;

        unsafe {
            // RCU read-side lock must be held between lookup and removal.
            urcu_sys::rcu_read_lock();

            let found_node = urcu_get_node::<Q, K, V>(self.urcuht, key);

            if !found_node.is_null() {
                found = true;
                // Return 0 if the node is successfully removed, negative value otherwise.
                // Deleting a NULL node or an already removed node will fail with a negative value.
                // Node can be looked up with cds_lfht_lookup and cds_lfht_next,
                // followed by use of cds_lfht_iter_get_node.

                // Call with rcu_read_lock held.
                // Threads calling this API need to be registered RCU read-side threads.
                err = urcu_sys::cds_lfht_del(self.urcuht, found_node);

                // Ask to free data after grace period
                let node = urcu_cds_lfht_node_to_rust_type::<K, V>(found_node);
                urcu_sys::urcu_memb_call_rcu(&mut (*node).head, Some(urcu_free_node::<K, V>));
            }

            urcu_sys::rcu_read_unlock();
        }

        if found {
            if err != 0 {
                Err(RcuError::DeleteError(err))
            } else {
                Ok(())
            }
        } else {
            Err(RcuError::NotFound)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::RcuHt;

    #[test]
    fn it_works() {
        // Type inference lets us omit an explicit type signature (which
        // would be `HashMap<String, String>` in this example).
        let ht = RcuHt::<String, String>::new(64, 64, 64, false)
            .expect("Cannot create hashtable, probably due to invalid parameters");
        let ht = std::sync::Arc::new(ht);
        // Create a new thread to get book reviews.
        let child = {
            let ht = ht.clone();
            std::thread::spawn(move || {
                {
                    // Get a read handle for this thread
                    let ht = ht.thread();
                    // wait until main thread adds some reviews
                    std::thread::sleep(std::time::Duration::from_millis(100));

                    let rdlock = ht.rdlock();

                    let review = rdlock.get("Adventures of Huckleberry Finn");
                    assert_eq!(review.is_some(), true);
                    let review = review.unwrap();
                    assert_eq!(review.eq("My favorite book."), true);
                }

                // uncomment this to check rdlock cannot live longer than ht thread
                /*
                let _out_error = {
                    // Get a read handle for this thread
                    let ht = ht.thread();

                    // get a rdlock and return it
                    ht.rdlock()
                };
                */

                // uncommend this to check wrlock cannot live longer than ht thread
                /*
                let _out_error = {
                    // Get a read handle for this thread
                    let ht = ht.thread();

                    // get a rdlock and return it
                    ht.wrlock()
                };
                */

                // uncomment this to check a ref to a value cannot live longer than rdlock
                /*
                // Get a read handle for this thread
                let ht = ht.thread();

                let _out_error = {
                    // get a rdlock
                    let rdlock = ht.rdlock();

                    // get a reference to a value then try to return it will lock is released
                    rdlock.get("Adventures of Huckleberry Finn")
                };
                */
            })
        };

        let ht = ht.thread();
        {
            let mut wrlock = ht.wrlock().unwrap();
            wrlock.insert_or_replace(
                "Adventures of Huckleberry Finn".to_string(),
                "My favorite book.".to_string(),
            );
        }

        {
            let mut wrlock = ht.wrlock().unwrap();
            wrlock.insert_or_replace(
                "Grimms' Fairy Tales".to_string(),
                "Masterpiece.".to_string(),
            );
        }

        child.join().expect("cannot join thread");

        // uncommend this to check RcuHtThread cannot live longer than RcuHt
        /*
        let _out_error = {
            // allocate a new hashtable
            let ht = RcuHt::<String,String>::new(64, 64, 64, false).unwrap();
            // Get a read handle for this thread
            let thread = ht.thread();

            // get a rdlock and return it
            thread
        };
        */
    }
}
