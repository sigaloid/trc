#![allow(dead_code)]
#![allow(clippy::mut_from_ref)]

use core::{
    borrow::{Borrow, BorrowMut},
    fmt::{Debug, Display, Pointer},
    ops::{Deref, DerefMut},
    pin::Pin,
    ptr::NonNull,
    hash::{Hash, Hasher},
};

#[cfg(any(
    all(not(target_has_atomic = "ptr"), feature = "default"),
    all(feature = "force_lock", not(feature = "nostd"))
))]
use std::sync::RwLock;

#[cfg(any(
    all(not(target_has_atomic = "ptr"), feature = "default"),
    all(feature = "force_lock", feature = "nostd")
))]
use spin::rwlock::RwLock;

#[cfg(any(
    all(target_has_atomic = "ptr", feature = "default"),
    all(target_has_atomic = "ptr", feature = "force_atomic")
))]
use core::sync::atomic::AtomicUsize;

pub struct SharedTrcData<T> {
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    atomicref: RwLock<usize>,
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    atomicref: AtomicUsize,
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    weakcount: RwLock<usize>,
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    weakcount: AtomicUsize,
    pub data: T,
}

struct LocalThreadTrcData<T> {
    shareddata: NonNull<SharedTrcData<T>>,
    threadref: usize,
}

/// `Trc<T>` is a heap-allocated smart pointer for sharing data across threads is a thread-safe and performant manner.
/// `Trc<T>` stands for: Thread Reference Counted.
/// `Trc<T>` provides shared ownership of the data similar to `Arc<T>` and `Rc<T>`. In addition, it also provides interior mutability.
/// It implements biased reference counting, which is based on the observation that most objects are only used by one thread.
/// This means that two reference counts can be created: one for thread-local use, and one atomic one for sharing between threads.
/// This implementation of biased reference counting sets the atomic reference count to the number of threads using the data.
///
/// ## Breaking reference cycles with `Weak<T>`
/// A cycle between `Trc` pointers cannot be deallocated as the reference counts will never reach zero. The solution is a `Weak<T>`.
/// A `Weak<T>` is a non-owning reference to the data held by a `Trc<T>`.
/// They break reference cycles by adding a layer of indirection and act as an observer. They cannot even access the data directly, and
/// must be converted back into `Trc<T>`. `Weak<T>` does not keep the value alive (whcih can be dropped), and only keeps the backing allocation alive.
/// See [`Weak`] for more information.
///
/// ## Clone behavior
/// When a `Trc<T>` is cloned, it's internal (wrapped) data stays at the same memory location, but a new `Trc<T>` is constructed and returned.
/// This makes a `clone` a relatively inexpensive operation because only a wrapper is constructed.
/// This new `Trc<T>` points to the same memory, and all `Trc<T>`s that point to that memory in that thread will have their thread-local reference counts incremented
/// and their atomic reference counts unchanged.
///
/// For use of threads, `Trc<T>` has a `clone_across_thread` method. This is relatively expensive; it allocates memory on the heap. However, calling the method
/// is most likely something that will not be done in loop.
/// `clone_across_thread` increments the atomic reference count - that is, the reference count that tells how many threads are using the object.
///
/// ## Drop behavior
///
/// When a `Trc<T>` is dropped the thread-local reference count is decremented. If it is zero, the atomic reference count is also decremented.
/// If the atomic reference count is zero, then the internal data is dropped. Regardless of wherether the atomic refernce count is zero, the
/// local `Trc<T>` is dropped.
///
/// ## [`Deref`] and [`DerefMut`] behavior
/// For ease of developer use, `Trc<T>` comes with [`Deref`] and [`DerefMut`] implemented to allow internal mutation.
/// `Trc<T>` automatically dereferences to `&T` or `&mut T`. This allows method calls and member acess of `T`.
/// To prevent name clashes, `Trc<T>`'s functions are associated.
///
/// ## Footnote on `dyn` wrapping
/// Rust's limitations mean that `Trc` will not be able to be used as a method reciever/trait object wrapper until
/// CoerceUnsized, DispatchFromDyn, and Reciever (with arbitrary_self_types) are stablized.
/// In addition, the internal structure of `Trc<T>` means that [`NonNull`] cannot be used as an indirection for CoerceUnsized due to it's
/// internals (`*const T`), and so wrapping `dyn` types cannot be implemented. Howeover, one can use a [`Box`] as a wrapper and then wrap with `Trc<T>`.
///
/// ## Examples
///
/// Example in a single thread:
/// ```
/// use trc::Trc;
///
/// let mut trc = Trc::new(100);
/// assert_eq!(*trc, 100);
/// *trc = 200;
/// assert_eq!(*trc, 200);
/// ```
///
/// Example with multiple threads:
/// ```
/// use std::thread;
/// use trc::Trc;
///
/// let trc = Trc::new(100);
/// let mut trc2 = Trc::clone_across_thread(&trc);
///
/// let handle = thread::spawn(move || {
///     *trc2 = 200;
/// });
///
/// handle.join().unwrap();
/// assert_eq!(*trc, 200);
/// ```
///
#[derive(Eq)]
pub struct Trc<T> {
    data: NonNull<LocalThreadTrcData<T>>,
}

impl<T> Trc<T> {
    /// Creates a new `Trc<T>` from the provided data.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc = Trc::new(100);
    /// assert_eq!(*trc, 100);
    /// ```
    #[inline]
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    pub fn new(value: T) -> Self {
        let shareddata = SharedTrcData {
            atomicref: AtomicUsize::new(1),
            weakcount: AtomicUsize::new(0),
            data: value,
        };

        let sbx = Box::new(shareddata);

        let localldata = LocalThreadTrcData {
            shareddata: NonNull::from(Box::leak(sbx)),
            threadref: 1,
        };

        let tbx = Box::new(localldata);

        Trc {
            data: NonNull::from(Box::leak(tbx)),
        }
    }

    /// Creates a new `Trc<T>` from the provided data.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc = Trc::new(100);
    /// assert_eq!(*trc, 100);
    /// ```
    #[inline]
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    pub fn new(value: T) -> Self {
        let shareddata = SharedTrcData {
            atomicref: RwLock::new(1),
            weakcount: RwLock::new(0),
            data: value,
        };

        let sbx = Box::new(shareddata);

        let localldata = LocalThreadTrcData {
            shareddata: NonNull::from(Box::leak(sbx)),
            threadref: 1,
        };

        let tbx = Box::new(localldata);

        Trc {
            data: NonNull::from(Box::leak(tbx)),
        }
    }

    /// Creates a new cyclic `Trc<T>` from the provided data. It allows the storage of `Weak<T>` which points the the allocation
    /// of `Trc<T>`inside of `T`. Holding a `Trc<T>` inside of `T` would cause a memory leak. This method works around this by
    /// providing a `Weak<T>` during the consturction of the `Trc<T>`, so that the `T` can store the `Weak<T>` internally.
    /// ```
    /// use trc::Trc;
    /// use trc::Weak;
    /// 
    /// struct T(Weak<T>);
    /// 
    /// let trc = Trc::new_cyclic(|x| T(x.clone()));
    /// ```
    #[inline]
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    pub fn new_cyclic<F>(data_fn: F) -> Self 
        where F: FnOnce(&Weak<T>) -> T
    {
        let shareddata: NonNull<_> = Box::leak(Box::new(SharedTrcData {
            atomicref: AtomicUsize::new(0),
            weakcount: AtomicUsize::new(0),
            data: std::mem::MaybeUninit::<T>::uninit(),
        })).into();

        let init_ptr: NonNull<SharedTrcData<T>> = shareddata.cast();
        
        let weak: Weak<_> = Weak {data: init_ptr};

        let data = data_fn(&weak);

        unsafe {
            let ptr = init_ptr.as_ptr();
            std::ptr::write(std::ptr::addr_of_mut!((*ptr).data), data);
            let prev = (*ptr)
                .atomicref
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            debug_assert_eq!(prev, 0);
        }

        let localldata = LocalThreadTrcData {
            shareddata: init_ptr,
            threadref: 1,
        };

        let tbx = Box::new(localldata);

        Trc {
            data: NonNull::from(Box::leak(tbx)),
        }
    }

    /// Creates a new cyclic `Trc<T>` from the provided data. It allows the storage of `Weak<T>` which points the the allocation
    /// of `Trc<T>`inside of `T`. Holding a `Trc<T>` inside of `T` would cause a memory leak. This method works around this by
    /// providing a `Weak<T>` during the consturction of the `Trc<T>`, so that the `T` can store the `Weak<T>` internally.
    /// ```
    /// use trc::Trc;
    /// use trc::Weak;
    /// 
    /// struct T(Weak<T>);
    /// 
    /// let trc = Trc::new_cyclic(|x| T(x.clone()));
    /// ```
    #[inline]
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    pub fn new_cyclic<F>(data_fn: F) -> Self 
        where F: FnOnce(&Weak<T>) -> T
    {
        let shareddata: NonNull<_> = Box::leak(Box::new(SharedTrcData {
            atomicref: RwLock::new(0),
            weakcount: RwLock::new(0),
            data: std::mem::MaybeUninit::<T>::uninit(),
        })).into();

        let init_ptr: NonNull<SharedTrcData<T>> = shareddata.cast();
        
        let weak: Weak<_> = Weak {data: init_ptr};

        let data = data_fn(&weak);

        unsafe {
            let ptr = init_ptr.as_ptr();
            std::ptr::write(std::ptr::addr_of_mut!((*ptr).data), data);
            let mut writelock = (*ptr).atomicref.try_write();

            #[cfg(not(feature = "nostd"))]
            {
                while writelock.is_err() {
                    writelock = (*ptr).atomicref.try_write();
                }
            }
            #[cfg(feature = "nostd")]
            {
                while writelock.is_none() {
                    writelock = (*ptr).atomicref.try_write();
                }
            }
            let mut writedata = writelock.unwrap();

            debug_assert_eq!(*writedata, 0);

            *writedata += 1;
        }

        let localldata = LocalThreadTrcData {
            shareddata: init_ptr,
            threadref: 1,
        };

        let tbx = Box::new(localldata);

        Trc {
            data: NonNull::from(Box::leak(tbx)),
        }
    }

    /// Return the thread-local reference count of the object, which is how many `Trc<T>`s in this thread point to the data referenced by this `Trc<T>`.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc = Trc::new(100);
    /// assert!(Trc::local_refcount(&trc) == 1)
    /// ```
    #[inline]
    pub fn local_refcount(this: &Self) -> usize {
        this.inner().threadref
    }

    /// Return the atomic reference count of the object. This is how many threads are using the data referenced by this `Trc<T>`.
    /// ```
    /// use std::thread;
    /// use trc::Trc;
    ///
    /// let trc = Trc::new(100);
    /// let mut trc2 = Trc::clone_across_thread(&trc);
    ///
    /// let handle = thread::spawn(move || {
    ///     *trc2 = 200;
    ///     assert_eq!(Trc::atomic_count(&trc2), 2);
    /// });
    ///
    /// handle.join().unwrap();
    /// assert_eq!(Trc::atomic_count(&trc), 1);
    /// assert_eq!(*trc, 200);
    /// ```
    #[inline]
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    pub fn atomic_count(this: &Self) -> usize {
        let mut readlock = this.inner_shared().atomicref.try_read();

        #[cfg(not(feature = "nostd"))]
        {
            while readlock.is_err() {
                readlock = this.inner_shared().atomicref.try_read();
            }
        }
        #[cfg(feature = "nostd")]
        {
            while readlock.is_none() {
                readlock = this.inner_shared().atomicref.try_read();
            }
        }
        *readlock.unwrap()
    }

    /// Return the atomic reference count of the object. This is how many threads are using the data referenced by this `Trc<T>`.
    /// ```
    /// use std::thread;
    /// use trc::Trc;
    ///
    /// let trc = Trc::new(100);
    /// let mut trc2 = Trc::clone_across_thread(&trc);
    ///
    /// let handle = thread::spawn(move || {
    ///     *trc2 = 200;
    ///     assert_eq!(Trc::atomic_count(&trc2), 2);
    /// });
    ///
    /// handle.join().unwrap();
    /// assert_eq!(Trc::atomic_count(&trc), 1);
    /// assert_eq!(*trc, 200);
    /// ```
    #[inline]
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    pub fn atomic_count(this: &Self) -> usize {
        this.inner_shared()
            .atomicref
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Return the weak count of the object. This is how many weak counts - across all threads - are pointing to the allocation inside of `Trc<T>`.
    /// ```
    /// use trc::Trc;
    /// use trc::Weak;
    ///
    /// let trc = Trc::new(100i32);
    /// let weak = Weak::from_trc(&trc);
    /// let weak2 = Weak::from_trc(&trc);
    /// let new_trc = Weak::to_trc(&weak).expect("Value was dropped");
    /// drop(weak);
    /// assert_eq!(Trc::weak_count(&new_trc), 1);
    /// ```
    #[inline]
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    pub fn weak_count(this: &Self) -> usize {
        let mut readlock = this.inner_shared().weakcount.try_read();

        #[cfg(not(feature = "nostd"))]
        {
            while readlock.is_err() {
                readlock = this.inner_shared().weakcount.try_read();
            }
        }
        #[cfg(feature = "nostd")]
        {
            while readlock.is_none() {
                readlock = this.inner_shared().weakcount.try_read();
            }
        }
        *readlock.unwrap()
    }

    /// Return the weak count of the object. This is how many weak counts - across all threads - are pointing to the allocation inside of `Trc<T>`.
    /// ```
    /// use trc::Trc;
    /// use trc::Weak;
    ///
    /// let trc = Trc::new(100i32);
    /// let weak = Weak::from_trc(&trc);
    /// let weak2 = Weak::from_trc(&trc);
    /// let new_trc = Weak::to_trc(&weak).expect("Value was dropped");
    /// drop(weak);
    /// assert_eq!(Trc::weak_count(&new_trc), 1);
    /// ```
    #[inline]
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    pub fn weak_count(this: &Self) -> usize {
        this.inner_shared()
            .weakcount
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Clone a `Trc<T>` across threads (increment it's atomic reference count). This is very important to do because it prevents reference count race conditions, which lead to memory errors.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc = Trc::new(100);
    /// let trc2 = Trc::clone_across_thread(&trc);
    /// assert_eq!(Trc::atomic_count(&trc), Trc::atomic_count(&trc2));
    /// ```
    #[inline]
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    pub fn clone_across_thread(this: &Self) -> Self {
        let mut writelock = this.inner_shared().atomicref.try_write();

        #[cfg(not(feature = "nostd"))]
        {
            while writelock.is_err() {
                writelock = this.inner_shared().atomicref.try_write();
            }
        }
        #[cfg(feature = "nostd")]
        {
            while writelock.is_none() {
                writelock = this.inner_shared().atomicref.try_write();
            }
        }
        let mut writedata = writelock.unwrap();

        *writedata += 1;

        let localldata = LocalThreadTrcData {
            shareddata: this.inner().shareddata,
            threadref: 1,
        };

        let tbx = Box::new(localldata);

        Trc {
            data: NonNull::from(Box::leak(tbx)),
        }
    }

    /// Clone a `Trc<T>` across threads (increment it's atomic reference count). This is very important to do because it prevents reference count race conditions, which lead to memory errors.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc = Trc::new(100);
    /// let trc2 = Trc::clone_across_thread(&trc);
    /// assert_eq!(Trc::atomic_count(&trc), Trc::atomic_count(&trc2));
    /// ```
    #[inline]
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    pub fn clone_across_thread(this: &Self) -> Self {
        this.inner_shared()
            .atomicref
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);

        let localldata = LocalThreadTrcData {
            shareddata: this.inner().shareddata,
            threadref: 1,
        };

        let tbx = Box::new(localldata);

        Trc {
            data: NonNull::from(Box::leak(tbx)),
        }
    }

    /// Checks if the other `Trc<T>` is equal to this one according to their internal pointers.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc1 = Trc::new(100);
    /// let trc2 = trc1.clone();
    /// assert!(Trc::ptr_eq(&trc1, &trc2));
    /// ```
    #[inline]
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.inner().shareddata.as_ptr() == other.inner().shareddata.as_ptr()
    }

    /// Gets the raw pointer to the most inner layer of `Trc<T>`.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc = Trc::new(100);
    /// println!("{}", Trc::as_ptr(&trc) as usize)
    /// ```
    #[inline]
    pub fn as_ptr(this: &Self) -> *mut SharedTrcData<T> {
        this.inner().shareddata.as_ptr()
    }

    /// Creates a new `Pin<Trc<T>>`. If `T` does not implement [`Unpin`], then the data will be pinned in memory and unable to be moved.
    #[inline]
    pub fn pin(data: T) -> Pin<Trc<T>> {
        unsafe { Pin::new_unchecked(Trc::new(data)) }
    }
}

impl<T> Trc<T> {
    #[inline]
    fn inner(&self) -> &LocalThreadTrcData<T> {
        unsafe { self.data.as_ref() }
    }

    #[inline]
    fn inner_shared(&self) -> &SharedTrcData<T> {
        unsafe { self.data.as_ref().shareddata.as_ref() }
    }

    #[inline]
    fn inner_mut(&self) -> &mut LocalThreadTrcData<T> {
        unsafe { &mut *self.data.as_ptr() }
    }

    #[inline]
    fn inner_shared_mut(&self) -> &mut SharedTrcData<T> {
        unsafe { &mut *(*self.data.as_ptr()).shareddata.as_ptr() }
    }
}

impl<T> Deref for Trc<T> {
    type Target = T;

    /// Get an immutable reference to the internal data.
    /// ```
    /// use trc::Trc;
    /// use std::ops::Deref;
    ///
    /// let mut trc = Trc::new(100i32);
    /// assert_eq!(*trc, 100i32);
    /// assert_eq!(trc.deref(), &100i32);
    /// ```
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner_shared().borrow().data
    }
}

impl<T> DerefMut for Trc<T> {
    /// Get a &mut reference to the internal data.
    /// ```
    /// use trc::Trc;
    /// use std::ops::DerefMut;
    ///
    /// let mut trc = Trc::new(100);
    /// *trc = 200;
    /// let mutref = trc.deref_mut();
    /// *mutref = 300;
    /// assert_eq!(*trc, 300);
    /// ```
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner_shared_mut().borrow_mut().data
    }
}

impl<T> Drop for Trc<T> {
    #[inline]
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    fn drop(&mut self) {
        self.inner_mut().threadref -= 1;
        if self.inner().threadref == 0 {
            let mut writelock = self.inner_shared().atomicref.try_write();

            #[cfg(not(feature = "nostd"))]
            {
                while writelock.is_err() {
                    writelock = self.inner_shared().atomicref.try_write();
                }
            }
            #[cfg(feature = "nostd")]
            {
                while writelock.is_none() {
                    writelock = self.inner_shared().atomicref.try_write();
                }
            }
            let mut writedata = writelock.unwrap();

            *writedata -= 1;

            if *writedata == 0 {
                std::mem::drop(writedata);

                let mut readlock = self.inner_shared().weakcount.try_read();

                #[cfg(not(feature = "nostd"))]
                {
                    while readlock.is_err() {
                        readlock = self.inner_shared().weakcount.try_read();
                    }
                }
                #[cfg(feature = "nostd")]
                {
                    while readlock.is_none() {
                        readlock = self.inner_shared().weakcount.try_read();
                    }
                }
                let readdata = readlock.unwrap();

                if *readdata > 0 {
                    unsafe { std::ptr::drop_in_place(&mut self.inner_shared_mut().data as *mut T) };
                    let new = unsafe {
                        NonNull::new_unchecked(std::mem::transmute_copy::<
                            usize,
                            *mut SharedTrcData<T>,
                        >(&usize::MAX))
                    };
                    self.inner_mut().shareddata = new;
                } else {
                    unsafe { Box::from_raw(self.inner().shareddata.as_ptr()) };
                    unsafe { Box::from_raw(self.data.as_ptr()) };
                }
            }
        }
    }

    #[inline]
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    fn drop(&mut self) {
        self.inner_mut().threadref -= 1;
        if self.inner().threadref == 0 {
            let res = self
                .inner_shared()
                .atomicref
                .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            if res > 0 && res - 1 == 0 {
                if self
                    .inner_shared()
                    .weakcount
                    .load(std::sync::atomic::Ordering::Acquire)
                    > 0
                {
                    unsafe { std::ptr::drop_in_place(&mut self.inner_shared_mut().data as *mut T) };
                } else {
                    unsafe { Box::from_raw(self.inner().shareddata.as_ptr()) };
                    unsafe { Box::from_raw(self.data.as_ptr()) };
                }
            }
        }
    }
}

impl<T> Clone for Trc<T> {
    /// Clone a `Trc<T>` (increment it's local reference count). This can only be used to clone an object that will only stay in one thread.
    /// If you need to clone in order to use a `Trc<T>` across threads, see [`clone_across_thread`](crate::trc::Trc#method.clone_across_thread).
    /// ```
    /// use trc::Trc;
    ///
    /// let trc = Trc::new(100);
    /// let trc2 = trc.clone();
    /// assert_eq!(Trc::local_refcount(&trc), Trc::local_refcount(&trc2));
    /// ```
    #[inline]

    fn clone(&self) -> Self {
        self.inner_mut().threadref += 1;

        Trc { data: self.data }
    }
}

impl<T> AsRef<T> for Trc<T> {
    fn as_ref(&self) -> &T {
        Trc::deref(self)
    }
}

impl<T> AsMut<T> for Trc<T> {
    fn as_mut(&mut self) -> &mut T {
        Trc::deref_mut(self)
    }
}

impl<T> Borrow<T> for Trc<T> {
    fn borrow(&self) -> &T {
        self.as_ref()
    }
}

impl<T> BorrowMut<T> for Trc<T> {
    fn borrow_mut(&mut self) -> &mut T {
        self.as_mut()
    }
}

impl<T: Default> Default for Trc<T> {
    fn default() -> Self {
        Trc::new(Default::default())
    }
}

impl<T: Display> Display for Trc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt((*self).deref(), f)
    }
}

impl<T: Debug> Debug for Trc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt((*self).deref(), f)
    }
}

impl<T: Pointer> Pointer for Trc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Pointer::fmt((*self).deref(), f)
    }
}

impl<T> From<T> for Trc<T> {
    /// Create a new `Trc<T>` from the provided data. This is equivalent to calling `Trc::new` on the same data.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc = Trc::from(100);
    /// assert_eq!(*trc, 100);
    /// ```
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: Hash> Hash for Trc<T> {
    /// Pass the data contained in this `Trc<T>` to the provided hasher.
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.deref().hash(state);
    }
}

impl<T: PartialOrd> PartialOrd for Trc<T> {
    /// "Greater than or equal to" comparison for two `Trc<T>`s. 
    /// 
    /// Calls `.partial_cmp` on the data.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc1 = Trc::from(100);
    /// let trc2 = Trc::from(100);
    /// assert!(trc1>=trc2);
    /// ```
    #[inline]
    fn ge(&self, other: &Self) -> bool {
        self.deref().ge(other.deref())
    }

    /// "Less than or equal to" comparison for two `Trc<T>`s. 
    /// 
    /// Calls `.le` on the data.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc1 = Trc::from(100);
    /// let trc2 = Trc::from(100);
    /// assert!(trc1<=trc2);
    /// ```
    #[inline]
    fn le(&self, other: &Self) -> bool {
        self.deref().ge(other.deref())
    }

    /// "Greater than" comparison for two `Trc<T>`s. 
    /// 
    /// Calls `.gt` on the data.
    /// ```
    /// use trc::Trc;
    /// 
    /// let trc1 = Trc::from(200);
    /// let trc2 = Trc::from(100);
    /// assert!(trc1>trc2);
    /// ```
    #[inline]
    fn gt(&self, other: &Self) -> bool {
        self.deref().gt(other.deref())
    }

    /// "Less than" comparison for two `Trc<T>`s. 
    /// 
    /// Calls `.lt` on the data.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc1 = Trc::from(100);
    /// let trc2 = Trc::from(200);
    /// assert!(trc1<trc2);
    /// ```
    #[inline]
    fn lt(&self, other: &Self) -> bool {
        self.deref().lt(other.deref())
    }

    /// Partial comparison for two `Trc<T>`s. 
    /// 
    /// Calls `.partial_cmp` on the data.
    /// ```
    /// use trc::Trc;
    /// use std::cmp::Ordering;
    ///
    /// let trc1 = Trc::from(100);
    /// let trc2 = Trc::from(200);
    /// assert_eq!(Some(Ordering::Less), trc1.partial_cmp(&trc2));
    /// ```
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.deref().partial_cmp(other.deref())
    }
}

impl<T: Ord> Ord for Trc<T> {
    /// Create a new `Trc<T>` from the provided data. This is equivalent to calling `Trc::new` on the same data.
    /// ```
    /// use trc::Trc;
    /// use std::cmp::Ordering;
    ///
    /// let trc1 = Trc::from(100);
    /// let trc2 = Trc::from(200);
    /// assert_eq!(Ordering::Less, trc1.cmp(&trc2));
    /// ```
    #[inline]
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.deref().cmp(other.deref())
    }
}

impl<T: PartialEq> PartialEq for Trc<T> {
    /// Equality by value comparison for two `Trc<T>`s, even if the data is in different allocoations. 
    /// 
    /// Calls `.eq` on the data.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc1 = Trc::from(100);
    /// let trc2 = Trc::from(100);
    /// assert!(trc1==trc2);
    /// ```
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.deref().eq(other.deref()) 
    }

    /// Equality by value comparison for two `Trc<T>`s, even if the data is in different allocoations. 
    /// 
    /// Calls `.ne` on the data.
    /// ```
    /// use trc::Trc;
    ///
    /// let trc1 = Trc::from(100);
    /// let trc2 = Trc::from(200);
    /// assert!(trc1!=trc2);
    /// ```
    #[allow(clippy::partialeq_ne_impl)]
    #[inline]
    fn ne(&self, other: &Self) -> bool {
        self.deref().ne(other.deref()) 
    }
}

unsafe impl<T: Sync + Send> Send for Trc<T> {}
unsafe impl<T: Sync + Send> Sync for Trc<T> {}

/// `Weak<T>` is a non-owning reference to `Trc<T>`'s data. It is used to prevent cyclic references which cause memory to never be freed.
/// `Weak<T>` does not keep the value alive (which can be dropped), they only keep the backing allocation alive. `Weak<T>` cannot even directly access the memory,
/// and must be converted into `Trc<T>` to do so.
///
/// One use case of a `Weak<T>`
/// is to create a tree: The parent nodes own the child nodes, and have strong `Trc<T>` references to their children. However, their children have `Weak<T>` references
/// to their parents.
///
/// To prevent name clashes, `Weak<T>`'s functions are associated.
///
/// # Examples
///
/// Example in a single thread:
/// ```
/// use trc::Trc;
/// use trc::Weak;
///
/// let trc = Trc::new(100);
/// let weak = Weak::from_trc(&trc);
/// let mut new_trc = Weak::to_trc(&weak).unwrap();
/// assert_eq!(*new_trc, 100);
/// *new_trc = 200;
/// assert_eq!(*new_trc, 200);
/// ```
///
/// Example with multiple threads:
/// ```
/// use std::thread;
/// use trc::Trc;
/// use trc::Weak;
///
/// let trc = Trc::new(100);
/// let weak = Weak::from_trc(&trc);
///
/// let handle = thread::spawn(move || {
///     let mut trc = Weak::to_trc(&weak).unwrap();
///     assert_eq!(*trc, 100);
///     *trc = 200;
/// });
/// handle.join().unwrap();
/// assert_eq!(*trc, 200);
/// ```
///
pub struct Weak<T> {
    data: NonNull<SharedTrcData<T>>, //Use this data because it has the ptr
}

impl<T> Weak<T> {
    /// Create a `Weak<T>` from a `Trc<T>`. This increments the weak count.
    ///
    /// ```
    /// use trc::Trc;
    /// use trc::Weak;
    ///
    /// let trc = Trc::new(100);
    /// let weak = Weak::from_trc(&trc);
    /// ```
    #[inline]
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    pub fn from_trc(trc: &Trc<T>) -> Weak<T> {
        let mut writelock = trc.inner_shared().weakcount.try_write();

        #[cfg(not(feature = "nostd"))]
        {
            while writelock.is_err() {
                writelock = trc.inner_shared().weakcount.try_write();
            }
        }
        #[cfg(feature = "nostd")]
        {
            while writelock.is_none() {
                writelock = trc.inner_shared().weakcount.try_write();
            }
        }
        let mut writedata = writelock.unwrap();

        *writedata += 1;
        Weak {
            data: unsafe { trc.data.as_ref() }.shareddata,
        }
    }

    /// Create a `Weak<T>` from a `Trc<T>`. This increments the weak count.
    ///
    /// ```
    /// use trc::Trc;
    /// use trc::Weak;
    ///
    /// let trc = Trc::new(100);
    /// let weak = Weak::from_trc(&trc);
    /// ```
    #[inline]
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    pub fn from_trc(trc: &Trc<T>) -> Weak<T> {
        trc.inner_shared()
            .weakcount
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Weak {
            data: unsafe { trc.data.as_ref() }.shareddata,
        }
    }

    /// Create a `Trc<T>` from a `Weak<T>`. Because `Weak<T>` does not own the value, it might have been dropped already. If it has, a `None` is returned.
    /// If the value has not been dropped, then this function a) decrements the weak count, and b) increments the atomic reference count of the object.
    ///
    /// ```
    /// use trc::Trc;
    /// use trc::Weak;
    ///
    /// let trc = Trc::new(100i32);
    /// let weak = Weak::from_trc(&trc);
    /// let new_trc = Weak::to_trc(&weak).expect("Value was dropped");
    /// drop(weak);
    /// assert_eq!(*new_trc, 100i32);
    /// ```
    #[inline]
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    pub fn to_trc(this: &Self) -> Option<Trc<T>> {
        let mut writelock = unsafe { this.data.as_ref() }.weakcount.try_write();

        #[cfg(not(feature = "nostd"))]
        {
            while writelock.is_err() {
                writelock = unsafe { this.data.as_ref() }.weakcount.try_write();
            }
        }
        #[cfg(feature = "nostd")]
        {
            while writelock.is_none() {
                writelock = unsafe { this.data.as_ref() }.weakcount.try_write();
            }
        }
        let mut writedata = writelock.unwrap();

        *writedata -= 1;

        let mut writelock = unsafe { this.data.as_ref() }.atomicref.try_write();

        #[cfg(not(feature = "nostd"))]
        {
            while writelock.is_err() {
                writelock = unsafe { this.data.as_ref() }.atomicref.try_write();
            }
        }
        #[cfg(feature = "nostd")]
        {
            while writelock.is_none() {
                writelock = unsafe { this.data.as_ref() }.atomicref.try_write();
            }
        }
        let mut writedata = writelock.unwrap();

        if *writedata == 0 {
            return None;
        }

        *writedata += 1;

        let localldata = LocalThreadTrcData {
            shareddata: this.data,
            threadref: 1,
        };

        let tbx = Box::new(localldata);

        Some(Trc {
            data: NonNull::from(Box::leak(tbx)),
        })
    }

    /// Create a `Trc<T>` from a `Weak<T>`. Because `Weak<T>` does not own the value, it might have been dropped already. If it has, a `None` is returned.
    /// If the value has not been dropped, then this function a) decrements the weak count, and b) increments the atomic reference count of the object.
    ///
    /// ```
    /// use trc::Trc;
    /// use trc::Weak;
    ///
    /// let trc = Trc::new(100i32);
    /// let weak = Weak::from_trc(&trc);
    /// let new_trc = Weak::to_trc(&weak).expect("Value was dropped");
    /// drop(weak);
    /// assert_eq!(*new_trc, 100i32);
    /// ```
    #[inline]
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    pub fn to_trc(this: &Self) -> Option<Trc<T>> {
        if unsafe { this.data.as_ref() }
            .atomicref
            .load(std::sync::atomic::Ordering::Acquire)
            == 0
        {
            return None;
        }

        unsafe { this.data.as_ref() }
            .weakcount
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        unsafe { this.data.as_ref() }
            .atomicref
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);

        let localldata = LocalThreadTrcData {
            shareddata: this.data,
            threadref: 1,
        };

        let tbx = Box::new(localldata);

        Some(Trc {
            data: NonNull::from(Box::leak(tbx)),
        })
    }
}

impl<T> Clone for Weak<T> {
    /// Clone a `Weak<T>` (increment the weak count).
    /// ```
    /// use trc::Trc;
    /// use trc::Weak;
    ///
    /// let trc = Trc::new(100);
    /// let weak1 = Weak::from_trc(&trc);
    /// let weak2 = weak1.clone();
    /// assert_eq!(Trc::weak_count(&trc), 2);
    /// ```
    #[inline]
    #[cfg(any(
        all(not(target_has_atomic = "ptr"), feature = "default"),
        feature = "force_lock"
    ))]
    fn clone(&self) -> Self {
        let mut writelock = unsafe { self.data.as_ref() }.weakcount.try_write();
        #[cfg(not(feature = "nostd"))]
        {
            while writelock.is_err() {
                writelock = unsafe { self.data.as_ref() }.weakcount.try_write();
            }
        }
        #[cfg(feature = "nostd")]
        {
            while writelock.is_none() {
                writelock = unsafe { self.data.as_ref() }.weakcount.try_write();
            }
        }
        let mut writedata = writelock.unwrap();

        *writedata += 1;

        Weak { data: self.data }
    }

    /// Clone a `Weak<T>` (increment the weak count).
    /// ```
    /// use trc::Trc;
    /// use trc::Weak;
    ///
    /// let trc = Trc::new(100);
    /// let weak1 = Weak::from_trc(&trc);
    /// let weak2 = weak1.clone();
    /// assert_eq!(Trc::weak_count(&trc), 2);
    /// ```
    #[inline]
    #[cfg(any(
        all(target_has_atomic = "ptr", feature = "default"),
        all(target_has_atomic = "ptr", feature = "force_atomic")
    ))]
    fn clone(&self) -> Self {
        unsafe { self.data.as_ref() }
            .weakcount
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);

        Weak { data: self.data }
    }
}

unsafe impl<T: Sync + Send> Send for Weak<T> {}
unsafe impl<T: Sync + Send> Sync for Weak<T> {}
