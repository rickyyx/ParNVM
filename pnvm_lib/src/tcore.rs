#[allow(unused_imports)]
use std::{
    any::Any,
    ops::Deref,
    sync::{
        atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
        Arc, Mutex, RwLock,
    },
};

use crossbeam::sync::ArcCell;

//use std::rc::Rc;
//use std::cell::RefCell;
//use tbox::TBox;
use txn::{Tid, TxnInfo};

#[cfg(feature = "pmem")]
use txn::PmemFac;

#[allow(unused_imports)]
use std::{
    self,
    cell::{RefCell, UnsafeCell},
    fmt, mem,
    ptr::Unique,
    rc::Rc,
    sync::{Once, ONCE_INIT},
    time,
};

//#[cfg(any(feature = "pmem", feature = "disk"))]
use pnvm_sys::{self, Alloc, AllocErr, Layout, PMEM_DEFAULT_SIZE, PMEM_FILE_DIR_BYTES};

#[cfg(feature = "profile")]
use flame;

use plog::PLog;

thread_local!{
    pub static COUNTER: RefCell<BenchmarkCounter> = RefCell::new(BenchmarkCounter::new());
}

#[derive(Clone, Debug)]
pub struct BenchmarkCounter {
    pub success_piece_cnt: u32,
    pub abort_piece_cnt:   u32,
    pub success_cnt:       u32,
    pub abort_cnt:         u32,
    pub new_order_cnt:     u32,
    pub get_time_cnt:      u32,
    pub mmap_cnt:          u32,
    pub pmem_flush_size:   u32,
    pub pmem_log_size:     u32,
    pub duration:          time::Duration,
    pub start:             time::Instant,
    pub avg_get_time:      time::Duration,
    pub success_over_time: Vec<u32>,
}

//#[cfg(benchmark)]
impl BenchmarkCounter {
    pub fn new() -> BenchmarkCounter {
        BenchmarkCounter {
            success_cnt:       0,
            abort_cnt:         0,
            success_piece_cnt: 0,
            abort_piece_cnt:   0,
            new_order_cnt:     0,
            get_time_cnt:      0,
            mmap_cnt:          0,
            pmem_flush_size:   0,
            pmem_log_size:     0,
            start:             time::Instant::now(),
            duration:          time::Duration::default(),
            avg_get_time:      time::Duration::default(),
            success_over_time: Vec::with_capacity(16),
        }
    }

    #[inline(always)]
    pub fn reset_cnt() {
        COUNTER.with(|c| {
            let c = &mut (*c.borrow_mut());
            c.success_cnt = 0;
            c.abort_cnt = 0;
            c.success_piece_cnt = 0;
            c.abort_piece_cnt = 0;
            c.new_order_cnt = 0;
            c.mmap_cnt = 0;
            c.pmem_flush_size = 0;
            c.pmem_log_size = 0;
            c.get_time_cnt = 0;
            c.start = time::Instant::now();
            c.success_over_time.clear();
        });
    }

    #[inline(always)]
    pub fn success() {
        COUNTER.with(|c| {
            (*c.borrow_mut()).success_cnt += 1;
        });
    }

    #[inline(always)]
    pub fn timestamp() {
        COUNTER.with(|c| {
            let c = &mut (*c.borrow_mut());
            c.success_over_time.push(c.success_cnt);
        });
    }

    #[inline(always)]
    pub fn flush(len: usize) {
        COUNTER.with(|c| {
            (*c.borrow_mut()).pmem_flush_size += len as u32;
        });
    }

    #[inline(always)]
    pub fn log(len: usize) {
        COUNTER.with(|c| {
            (*c.borrow_mut()).pmem_log_size += len as u32;
        });
    }

    #[inline(always)]
    pub fn get_time() {
        COUNTER.with(|c| {
            (*c.borrow_mut()).get_time_cnt += 1;
        });
    }

    #[inline(always)]
    pub fn set_get_time(t: time::Duration) {
        COUNTER.with(|c| {
            (*c.borrow_mut()).avg_get_time = t;
        });
    }

    #[inline(always)]
    pub fn mmap() {
        COUNTER.with(|c| {
            (*c.borrow_mut()).mmap_cnt += 1;
        });
    }

    #[inline(always)]
    pub fn success_piece() {
        COUNTER.with(|c| {
            (*c.borrow_mut()).success_piece_cnt += 1;
        });
    }

    #[inline(always)]
    pub fn abort_piece() {
        COUNTER.with(|c| {
            (*c.borrow_mut()).abort_piece_cnt += 1;
        });
    }

    pub fn new_order_done() {
        COUNTER.with(|c| {
            (*c.borrow_mut()).new_order_cnt += 1;
        });
    }

    #[inline(always)]
    pub fn start() {
        COUNTER.with(|c| c.borrow_mut().start = time::Instant::now())
    }

    #[inline(always)]
    pub fn abort() {
        COUNTER.with(|c| {
            (*c.borrow_mut()).abort_cnt += 1;
        });
    }

    #[inline(always)]
    pub fn copy() -> BenchmarkCounter {
        COUNTER.with(|c| {
            let mut g = c.borrow_mut();
            let dur = g.start.elapsed();
            g.duration = dur;
            g.clone()
        })
    }

    #[inline(always)]
    pub fn add_time(dur: time::Duration) {
        COUNTER.with(|c| (*c.borrow_mut()).duration += dur)
    }
}

pub trait BoxRef<T> {
    fn into_box_ref(self) -> Box<dyn TRef>;
}

pub trait TRef: fmt::Debug {
    fn get_ptr(&self) -> *mut u8;
    fn get_field_ptr(&self, usize) -> *mut u8;
    fn get_field_size(&self, usize) -> usize;
    fn get_layout(&self) -> Layout;
    fn install(&self, id: Tid);
    fn box_clone(&self) -> Box<dyn TRef>;
    fn get_id(&self) -> &ObjectId;
    fn get_tvers(&self) -> &Arc<TVersion>;
    fn get_version(&self) -> u32;
    fn read(&self) -> &Any;

    //TODO: wdrain
    #[cfg(not(all(feature = "wdrain", feature = "pmem")))]
    fn write(&mut self, Box<Any>);

    #[cfg(all(feature = "wdrain", feature = "pmem"))]
    fn write(&mut self, *mut u8);

    fn lock(&self, Tid) -> bool;
    fn unlock(&self);
    fn check(&self, u32, u32) -> bool;
    fn get_access_info(&self) -> Arc<TxnInfo>;
    fn set_access_info(&mut self, Arc<TxnInfo>);
    fn get_name(&self) -> String;

    /* 2PL locking functions */
    fn read_lock(&self, u32) -> bool;
    fn read_unlock(&self, u32);
    fn write_lock(&self, u32) -> bool;
    fn write_unlock(&self, u32);
    fn write_through(&self, Box<Any>, Tid);

    #[cfg(any(feature = "pmem", feature = "disk"))]
    fn get_pmem_addr(&self) -> *mut u8;
    #[cfg(any(feature = "pmem", feature = "disk"))]
    fn get_pmem_field_addr(&self, usize) -> *mut u8;
}

#[derive(PartialEq, Copy, Clone, Debug, Eq, Hash)]
pub struct ObjectId(u64);

//[TODO:]To be optimized later
//Ideally the TVersion should be generic over variuos concurrecy protocols
//E.g.: TVersion<C = OCC>, and core implementation of TVersion should be done
//through various concurrency struct (C)
#[derive(Debug)]
pub struct TVersion {
    pub last_writer_: AtomicU32,
    pub lock_owner_:  AtomicU32,
    pub txn_info_:    ArcCell<TxnInfo>, /* Info of the last writer's txn_ info */

    pub count_: AtomicU32, /* This to allow multiple times of locking */

    /* For two phase locking(tpl)'s constructs */
    tpl_cr_:         AtomicBool, //Mutex for updating
    tpl_reader_:     AtomicU32,  //current max reader
    tpl_reader_cnt_: AtomicU32,  //Reader count
    tpl_writer_:     AtomicU32,  //current writer
}

impl TVersion {
    pub fn new_with_info(txn_info: Arc<TxnInfo>) -> TVersion {
        TVersion {
            last_writer_:    AtomicU32::new(txn_info.id().into()),
            lock_owner_:     AtomicU32::new(0),
            txn_info_:       ArcCell::new(txn_info),
            count_:          AtomicU32::new(0),
            tpl_cr_:         AtomicBool::new(false),
            tpl_writer_:     AtomicU32::new(0),
            tpl_reader_:     AtomicU32::new(0),
            tpl_reader_cnt_: AtomicU32::new(0),
        }
    }

    //Interface for OCC based concurrency control
    //lock: exclusive lock on the write-set
    //unlock;
    //check_version: check on read set
    #[inline(always)]
    pub fn lock(&self, tid: Tid) -> bool {
        let tid: u32 = tid.into();
        debug_assert!(tid != 0, true);
        match self
            .lock_owner_
            .compare_exchange(0, tid, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_cur) => {
                debug_assert!(self.count_.load(Ordering::Acquire) == 0, true);
                self.count_.fetch_add(1, Ordering::AcqRel);
                true
            }
            Err(cur) => {
                if cur == tid {
                    /* Lock by me, and it is safe to do the fetch_add since no one else will
                     * be able to access this */
                    debug_assert!(self.count_.load(Ordering::Acquire) > 0, true);
                    self.count_.fetch_add(1, Ordering::AcqRel);
                    true
                } else {
                    /* Lock by others */
                    false
                }
            }
        }
    }

    //WARNING: whoever has access to self can unlock
    #[inline(always)]
    pub fn unlock(&self) {
        if self.count_.fetch_sub(1, Ordering::AcqRel) == 1 {
            debug_assert!(self.lock_owner_.load(Ordering::Acquire) != 0, true);
            debug_assert!(self.count_.load(Ordering::Acquire) == 0, true);
            self.lock_owner_.store(0, Ordering::Release);
        }
    }

    pub fn get_locker(&self) -> u32 {
        self.lock_owner_.load(Ordering::Relaxed)
    }

    pub fn get_count(&self) -> u32 {
        self.count_.load(Ordering::Relaxed)
    }

    //Check the version is not write locked by others and the most recent true version
    //matches with the local snapshot of the transaction (cur)
    #[inline(always)]
    pub fn check_version(&self, cur: u32, tid: u32) -> bool {
        ((self.lock_owner_.load(Ordering::Acquire) == 0
            || self.lock_owner_.load(Ordering::Acquire) == tid)
            && self.last_writer_.load(Ordering::Acquire) == cur)
    }

    //#[cfg_attr(feature = "profile", flame)]
    #[inline(always)]
    pub fn get_version(&self) -> u32 {
        self.last_writer_.load(Ordering::Acquire)
    }

    #[inline(always)]
    pub fn set_version(&self, tid: u32) {
        self.last_writer_.store(tid, Ordering::Release)
    }

    #[inline(always)]
    pub fn get_access_info(&self) -> Arc<TxnInfo> {
        self.txn_info_.get()
    }

    #[inline(always)]
    pub fn set_access_info(&self, txn_info: Arc<TxnInfo>) {
        self.txn_info_.set(txn_info);
    }

    /* Interface for the 2PL */
    pub fn read_lock(&self, tid: u32) -> bool {
        let mut count: u64 = 0;
        loop {
            //Enter Reader updating CR
            self.enter_cr(tid);

            match self.tpl_writer_.load(Ordering::SeqCst) {
                0 => {
                    self.tpl_reader_.fetch_max(tid, Ordering::SeqCst);
                    self.tpl_reader_cnt_.fetch_add(1, Ordering::SeqCst);
                    self.exit_cr();
                    return true;
                }
                writer => {
                    //Wait-die Ddlck prevention
                    if writer < tid {
                        self.exit_cr();
                        return false;
                    } else if writer == tid {
                        self.tpl_reader_.fetch_max(tid, Ordering::SeqCst);
                        self.tpl_reader_cnt_.fetch_add(1, Ordering::SeqCst);
                        self.exit_cr();
                        return true;
                    } else {
                        /* NO-OP for writer < tid */
                        count += 1;

                        /* For debug */
                        if count == 100_000_000 {
                            println!("spinning in read lock for too long");
                        }
                        if count >= 200_000_000 {
                            panic!("spinning in read lock for too long");
                        }
                        self.exit_cr();
                        std::thread::yield_now();
                    }
                }
            }
        }
    }

    //TODO:
    //Upgrading from read lock to write lock is currently not fully supported
    pub fn read_unlock(&self, tid: u32) {
        self.enter_cr(tid);
        if self.tpl_reader_cnt_.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.tpl_reader_.store(0, Ordering::SeqCst);
        }
        self.exit_cr();
    }

    //DO NOT Allow Recursive write locks
    //Wlock
    pub fn write_lock(&self, tid: u32) -> bool {
        let mut count: u64 = 0;
        'start: loop {
            self.enter_cr(tid);

            /* Check writer */
            let cur_writer = self.tpl_writer_.load(Ordering::SeqCst);
            match cur_writer {
                0 => {} /* Fall thruogh */
                blocker => {
                    /* Wait die ddl prevention */
                    if blocker < tid {
                        self.exit_cr();
                        return false;
                    } else if blocker == tid {
                        self.exit_cr();
                        //TODO:
                        //Currently recursive wlock on the same object by same txn
                        //is not allowed
                        println!("recusrive wlock {}", tid);
                        panic!();
                    } else {
                        /* Wait for cur writer to release */
                        self.exit_cr();
                        std::thread::yield_now();
                        count += 1;
                        if count == 100_000_000 {
                            warn!(
                                "spinning in write_lock - blocker: {:?}, tid: {:?}",
                                blocker, tid
                            );
                        }
                        if count >= 200_000_000 {
                            panic!(
                                "spinning in write_lock - blocker: {:?}, tid: {:?}",
                                blocker, tid
                            );
                        }
                        continue 'start;
                    }
                }
            }

            /* Check reader */
            match self.tpl_reader_.load(Ordering::SeqCst) {
                0 => {
                    assert_eq!(self.tpl_reader_cnt_.load(Ordering::SeqCst) == 0, true);
                    self.tpl_writer_.store(tid, Ordering::SeqCst);
                    self.exit_cr();
                    return true;
                }
                max_reader => {
                    //Wait-die Dlck prevention
                    if max_reader > tid {
                        self.exit_cr();
                        return false;
                    } else if max_reader == tid {
                        assert_eq!(self.tpl_writer_.load(Ordering::SeqCst), 0);
                        self.tpl_writer_.store(tid, Ordering::SeqCst);

                        while self.tpl_reader_cnt_.load(Ordering::SeqCst) != 1 {
                            self.exit_cr();
                            count += 1;
                            if count >= 100_000_000 {
                                panic!(
                                    "spinning in write_lock - max_reader: {:?}, tid: {:?}",
                                    max_reader, tid
                                );
                            }
                            std::thread::yield_now();
                            self.enter_cr(tid);
                        }

                        //In the critical section
                        self.exit_cr();
                        return true;
                    } else {
                        // No op if I should be waiting
                        count += 1;
                        if count >= 100_000_000 {
                            panic!(
                                "spinning in write_lock - max_reader: {:?}, tid: {:?}",
                                max_reader, tid
                            );
                        }
                    }
                }
            }
            self.exit_cr();
        }
    }

    pub fn write_unlock(&self, tid: u32) {
        self.enter_cr(tid);
        //Multiple unlock might be called
        self.tpl_writer_
            .compare_exchange(tid, 0, Ordering::SeqCst, Ordering::SeqCst)
            .expect("Write lock poisoned");
        self.exit_cr();
    }

    //Spin lock on entering the critical section
    fn enter_cr(&self, tid: u32) {
        let mut count: u64 = 0;
        while self.tpl_cr_.compare_and_swap(false, true, Ordering::SeqCst) {
            count += 1;
            if count >= 100_000_000 {
                panic!("spinning enter cr {:?}", tid);
            }
        }
    }

    fn exit_cr(&self) {
        self.tpl_cr_
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .expect("Critical Session invalid");
    }
}

impl Default for TVersion {
    fn default() -> Self {
        TVersion {
            last_writer_: AtomicU32::new(0),
            lock_owner_:  AtomicU32::new(0),
            txn_info_:    ArcCell::new(Arc::new(TxnInfo::default())),
            count_:       AtomicU32::new(0),

            tpl_cr_:         AtomicBool::new(false),
            tpl_writer_:     AtomicU32::new(0),
            tpl_reader_:     AtomicU32::new(0),
            tpl_reader_cnt_: AtomicU32::new(0),
        }
    }
}

#[derive(Debug)]
pub struct TValue<T>
where
    T: Clone,
{
    data_: AtomicPtr<T>,
}

impl<T> TValue<T>
where
    T: Clone,
{
    pub fn new(val: T) -> TValue<T> {
        #[cfg(feature = "pmem")]
        {
            #[cfg(any(feature = "wdrain", feature = "dir"))]
            {
                let mut ptr = PmemFac::alloc(mem::size_of::<T>()) as *mut T;
                unsafe { ptr.write(val) };

                TValue {
                    data_: AtomicPtr::new(ptr),
                }
            }

            #[cfg(not(any(feature = "wdrain", feature = "dir")))]
            {
                TValue {
                    data_: AtomicPtr::new(Box::into_raw(Box::new(val))),
                }
            }
        }

        #[cfg(not(feature = "pmem"))]
        {
            TValue {
                data_: AtomicPtr::new(Box::into_raw(Box::new(val))),
            }
        }
    }

    #[cfg(all(feature = "pmem", feature = "wdrain"))]
    pub fn store(&self, ptr: *mut T) {
        let old = self.data_.swap(ptr, Ordering::SeqCst);
        //unsafe {drop_in_place(old)};
    }

    #[cfg(not(all(feature = "pmem", feature = "wdrain")))]
    pub fn store(&self, data: T) {
        let ptr = Box::into_raw(Box::new(data));
        let _old = self.data_.swap(ptr, Ordering::SeqCst);
    }

    pub fn load(&self) -> &T {
        //unsafe { self.ptr_.as_ref() }
        unsafe { &*(self.data_.load(Ordering::SeqCst)) }
        //unsafe { &*self.data_.get()}
    }

    pub fn get_ptr(&self) -> *mut T {
        self.data_.load(Ordering::SeqCst)
    }
}

pub type FieldArray = Vec<usize>;

pub struct TTag {
    pub tobj_ref_:  Box<dyn TRef>,
    pub oid_:       ObjectId,
    pub has_write_: bool,
    pub fields_:    Option<FieldArray>, /* Fix length of the fields idx buffer */
    is_lock_:       bool,
    pub vers_:      u32, /* 0 means empty */

    //for debug
    pub name_: String,
}

impl TTag {
    pub fn new(oid: ObjectId, tobj_ref: Box<dyn TRef>) -> Self {
        TTag {
            oid_:      oid,
            name_:     tobj_ref.get_name(),
            tobj_ref_: tobj_ref,
            //write_val_: None,
            vers_:      0,
            has_write_: false,
            is_lock_:   false,
            fields_:    None,
        }
    }

    pub fn commit_data(&mut self, id: Tid) {
        if !self.has_write() {
            return;
        }

        self.tobj_ref_.install(id);
    }

    pub fn get_data<T: 'static>(&self) -> &T {
        match self.tobj_ref_.read().downcast_ref::<T>() {
            Some(t_ref) => t_ref,
            None => panic!("inconsistent data {:?}", self),
        }
    }

    pub fn lock(&mut self, tid: Tid) -> bool {
        if self.tobj_ref_.lock(tid) {
            self.is_lock_ = true;
            true
        } else {
            warn!("[{:?}] LOCKED Failed :{}", tid, self.tobj_ref_.get_name());
            false
        }
    }

    pub fn is_lock(&self) -> bool {
        self.is_lock_
    }

    pub fn unlock(&mut self) {
        self.tobj_ref_.unlock();
        self.is_lock_ = false;
    }

    pub fn check(&self, vers: u32, tid: u32) -> bool {
        self.tobj_ref_.check(vers, tid)
    }

    pub fn set_write(&mut self) {
        self.has_write_ = true;
    }

    pub fn set_fields(&mut self, fields: FieldArray) {
        self.fields_ = Some(fields);
    }

    //#[cfg_attr(feature = "profile", flame)]
    #[inline(always)]
    pub fn has_write(&self) -> bool {
        self.has_write_
    }

    #[inline(always)]
    pub fn has_read(&self) -> bool {
        !self.has_write()
    }

    //#[cfg_attr(feature = "profile", flame)]
    #[inline(always)]
    pub fn add_version(&mut self, vers: u32) {
        self.vers_ = vers;
    }

    pub fn get_version(&self) -> u32 {
        self.tobj_ref_.get_version()
    }

    #[inline(always)]
    #[cfg(not(all(feature = "wdrain", feature = "pmem")))]
    pub fn write<T: 'static>(&mut self, val: T) {
        let val = Box::new(val);
        self.tobj_ref_.write(val);
        self.has_write_ = true;
    }

    #[cfg(all(feature = "wdrain", feature = "pmem"))]
    pub fn write<T: 'static>(&mut self, mut val: T) {
        let size = mem::size_of::<T>();
        let mut pmem_ptr = PmemFac::alloc(size) as *mut T;
        pnvm_sys::memcpy_nodrain(pmem_ptr as *mut u8, &mut val as *mut _ as *mut u8, size);

        self.tobj_ref_.write(pmem_ptr as *mut u8);
        self.has_write_ = true;
    }

    pub fn persist_data(&self, _: Tid) {
        if !self.has_write() {
            return;
        }

        #[cfg(feature = "pmem")]
        match self.fields_ {
            Some(ref fields) => {
                for field in fields.iter() {
                    let pmemaddr = self.tobj_ref_.get_pmem_field_addr(*field);
                    let size = self.tobj_ref_.get_field_size(*field);
                    let vaddr = self.tobj_ref_.get_field_ptr(*field);
                    warn!(
                        "[{:?}] [persit_data] name : {:?}, paddr: {:p}, vaddr: {:p}, size: {}",
                        self.tobj_ref_.get_id(),
                        self.name_,
                        pmemaddr,
                        vaddr,
                        size
                    );
                    BenchmarkCounter::flush(size);

                    #[cfg(feature = "dir")]
                    {
                        pnvm_sys::flush(pmemaddr, size);
                    }

                    #[cfg(not(feature = "dir"))]
                    {
                        pnvm_sys::memcpy_nodrain(pmemaddr, vaddr, size);
                    }
                }
            }
            None => {
                let pmemaddr = self.tobj_ref_.get_pmem_addr();
                let layout = self.tobj_ref_.get_layout();

                BenchmarkCounter::flush(layout.size());
                #[cfg(not(feature = "dir"))]
                {
                    pnvm_sys::memcpy_nodrain(
                        pmemaddr,
                        self.tobj_ref_.get_ptr(),
                        self.tobj_ref_.get_layout().size(),
                    );
                }

                #[cfg(feature = "dir")]
                pnvm_sys::flush(pmemaddr, layout.size());
            }
        }

        #[cfg(feature = "disk")]
        {
            pnvm_sys::disk_memcpy(
                pmemaddr,
                self.tobj_ref_.get_ptr(),
                self.tobj_ref_.get_layout().size(),
            );

            pnvm_sys::disk_msync(pmemaddr, self.tobj_ref_.get_layout().size());
        }
    }

    pub fn make_log(&self, id: Tid) -> PLog {
        PLog::new(
            self.tobj_ref_.get_ptr() as *mut u8,
            self.tobj_ref_.get_layout(),
            id,
        )
    }

    #[cfg(any(feature = "pmem", feature = "disk"))]
    pub fn make_record(&self) -> (*mut u8, *mut u8, Layout) {
        (
            self.tobj_ref_.get_pmem_addr(),
            self.tobj_ref_.get_ptr(),
            self.tobj_ref_.get_layout(),
        )
    }
}

impl fmt::Debug for TTag {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "TTag {{  Oid: {:?} ,  Vers : {:?}}}",
            self.oid_, self.vers_
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Operation {
    RWrite,
    Delete,
    Push,
}

/* Object ID factory */
static mut OBJECTID: u64 = 1;
pub unsafe fn next_id() -> ObjectId {
    let ret = OBJECTID;
    OBJECTID += 1;
    ObjectId(ret)
}

thread_local! {
    pub static OID_FAC : Rc<RefCell<OidFac>> = Rc::new(RefCell::new(OidFac::new()));
}

pub struct OidFac {
    mask_:    u64,
    next_id_: u64,
}

impl OidFac {
    /* Thread Local methods */
    pub fn set_obj_mask(mask: u64) {
        OID_FAC.with(|fac| fac.borrow_mut().set_mask(mask))
    }

    /* Thread Local methods */
    pub fn get_obj_next() -> ObjectId {
        OID_FAC.with(|fac| fac.borrow_mut().get_next())
    }

    pub fn new() -> OidFac {
        OidFac {
            mask_:    0,
            next_id_: 1,
        }
    }

    fn set_mask(&mut self, mask: u64) {
        self.mask_ = mask;
    }

    fn get_next(&mut self) -> ObjectId {
        let ret = self.next_id_ | ((self.mask_) << 52);
        self.next_id_ += 1;
        ObjectId(ret)
    }
}

/*
 * Persistent Memory Allocator
 */
//#[cfg(feature = "pmem")]
//static mut G_PMEM_ALLOCATOR: PMem = PMem {
//    kind: 0 as *mut MemKind,
//    size: 0,
//};
//
//#[cfg(feature = "pmem")]
//fn get_pmem_allocator() -> PMem {
//    unsafe {
//        if G_PMEM_ALLOCATOR.kind as u32 == 0 {
//            G_PMEM_ALLOCATOR =
//                PMem::new_bytes_with_nul_unchecked(PMEM_FILE_DIR_BYTES, PMEM_DEFAULT_SIZE);
//        }
//        G_PMEM_ALLOCATOR
//    }
//}
//
//#[cfg(feature = "pmem")]
//pub struct GPMem;
//
//#[cfg(feature = "pmem")]
//unsafe impl GlobalAlloc for GPMem {
//    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
//        let mut pmem = get_pmem_allocator();
//        pmem.alloc(layout).unwrap()
//    }
//
//    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
//        let mut pmem = get_pmem_allocator();
//        pmem.dealloc(ptr, layout)
//    }
//}
