use std::sync::{ Arc, RwLock,Mutex};
//use std::rc::Rc;
//use std::cell::RefCell;
use txn::{Tid};
use std::{
    self,
    fmt,
    ptr::NonNull,
    mem,
    rc::Rc,
};
use pnvm_sys::{
    self,
    Layout,
};

/* Module Level Exposed Function Calls */









//Base trait for all the data structure
pub type TObject<T> = Arc<RwLock<_TObject<T>>>;


pub trait _TObject<T> 
where T: Clone 
    {
    fn lock(&mut self, Tid) -> bool;
    fn check(&self, &Option<Tid>) -> bool;
    fn install(&mut self, T, Tid);
    fn unlock(&mut self);
    fn get_id(&self) -> ObjectId;
    fn get_data(&self) -> T;
    fn get_version(&self) -> Option<Tid>;

    //For debug
    fn raw_read(&self) -> T;
    fn raw_write(&mut self, T) ;
}


#[derive(PartialEq,Copy, Clone, Debug, Eq, Hash)]
pub struct ObjectId(u32);


//[TODO:]To be optimized later
pub struct TVersion {
    pub last_writer_: Option<Tid>,
    //lock_:        Arc<Mutex<bool>>,
    pub lock_owner_:  Mutex<Option<Tid>>
    //lock_owner_:  Option<Tid>,
}


//TTag is attached with each logical segment (identified by key)
//for a TObject. 
//TTag is a local object to the thread.

impl TVersion {
    pub fn lock(&mut self, tid: Tid) -> bool {
        let mut lock_owner = self.lock_owner_.lock().unwrap();
        let (success, empty) = match *lock_owner {
            Some(ref cur_owner) => {
                if *cur_owner == tid {
                    (true, false)
                } else {
                    (false, false)
                }
            },
            None => {
                (true, true)
            }
        };
        if empty {
            *lock_owner = Some(tid)
        }
        success
    }
    

    //Caution: whoever has access to self can unlock
    pub fn unlock(&mut self) {
        let mut lock_owner = self.lock_owner_.lock().unwrap();
        *lock_owner = None;
    }

    pub fn check_version(&self, tid: &Option<Tid>) -> bool {
        println!("--- [Checking Version] {:?} <-> {:?}", tid, self.last_writer_);
        let lock_owner = self.lock_owner_.lock().unwrap();
        match (tid, self.last_writer_, *lock_owner) {
            (Some(ref cur_tid), Some(ref tid), None) => {
                if *cur_tid == *tid {
                    true
                } else {
                    false 
                }
            },
            (None, None, None)  => true,
            (_ , _, _) => false
        }
    }

    //What if the last writer is own? -> Extension
    pub fn get_version(&self) -> Option<Tid> {
        self.last_writer_ 
    }

    pub fn set_version(&mut self, tid: Tid) {
        self.last_writer_ = Some(tid);
    }
}

pub struct TValue<T>
where T:Clone {
    ptr_: NonNull<T>,
}

impl<T> TValue<T> 
where T:Clone
{
    pub fn new(val :T) -> TValue<T> {
        let ptr = unsafe { pnvm_sys::alloc(Layout::new::<T>())};

        match ptr {
            Ok(ptr) => {
                let ptr = unsafe {
                    mem::transmute::<*mut u8, *mut T>(ptr)
                };
                unsafe {ptr.write(val)};
                TValue{ 
                    ptr_ : NonNull::new(ptr).expect("Tvalue::new failed"),
                }
            },
            Err(_) => panic!("Tvalue::new failed")
        }
    }
    pub fn store(&mut self, data: T) {
        unsafe {self.ptr_.as_ptr().write(data) };
    }

    pub fn load(&self) -> &T {
        unsafe {self.ptr_.as_ref()}
    }
   

    //FIXME::This is super dangerous...
    //But it might be a feasible option. Wrapping the underlying data with 
    //Rc<RefCell<T>> could be a way to pass the data as a ref all
    //the way up to the user. A main intended advantage is to avoid 
    //copying the underlying data. 
    //However, there seems to be no direct methods that place
    //data from a pointer to a refcell. 
    //
    pub fn get_ref(&self) -> Rc<T> {
        unsafe {Rc::from_raw(self.ptr_.as_ref())}        
    }
}

//#[derive(PartialEq, Eq, Hash)]
pub struct TTag<T> {
    pub tobj_ref_:  TObject<T>,
    pub oid_:   ObjectId,
    write_val_: Option<T>,
    pub vers_ : Option<Tid>
}

impl<T> TTag<T>
where T:Clone
{
    pub fn new(oid: ObjectId, tobj_ref: TObject<T>) -> Self {
        TTag{
            oid_: oid,
            tobj_ref_ : tobj_ref,
            write_val_: None,
            vers_ : None
        }
    }

    pub fn write_value(&self) -> T {
        match self.write_val_ {
            Some(ref t) => T::clone(t),
            None => panic!("Write Tag Should Have Write Value")
        }
    }

    pub fn commit(&mut self, id : Tid) {
        if !self.has_write() {
            return;
        }

        let val = self.write_value();
        let mut _tobj = self.tobj_ref_.write().unwrap();
        _tobj.install(val, id);
    }

   // pub fn consume_value(&mut self) -> T {
   //     match self.write_val_ {
   //         Some(t) => Rc::try_unwrap(t).ok().unwrap(),
   //         None => panic!("Write Tag Should Have Write Value")
   //     }
   // }

    pub fn has_write(&self) -> bool {
        match self.write_val_ {
            Some(_) => true,
            None => false
        }
    }

    pub fn has_read(&self) -> bool {
        !self.has_write()
    }

    pub fn add_version(&mut self, vers: Option<Tid>) {
        self.vers_ = vers;
    }

    pub fn write(&mut self, val : T) {
        self.write_val_ = Some(val)
    }
}

impl<T> fmt::Debug for TTag<T> 
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TTag {{  Oid: {:?} ,  Vers : {:?}}}", self.oid_,  self.vers_)
    }
}


static mut OBJECTID: u32 = 1;
pub unsafe fn next_id() -> ObjectId {
    let ret = OBJECTID;
    OBJECTID += 1;
    ObjectId(ret)
}



