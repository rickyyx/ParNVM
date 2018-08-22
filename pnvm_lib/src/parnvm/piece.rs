use plog::PLog;
use std::fmt::{Debug, Formatter, Result};
use std::sync::Arc;
use txn::Tid;

use std::mem;

use super::nvm_txn::{TransactionPar, TransactionParOCC};

//FIXME: core
use core::alloc::Layout;

#[derive(Eq, PartialEq, Hash, Debug, Clone, Copy)]
pub struct Pid(u32);

impl Pid {
    pub fn new(pid: u32) -> Pid {
        Pid(pid)
    }
}

type FnPtr = Arc<Box<Fn(&mut TransactionPar) -> i32 + Send + Sync>>;

type FnPtrOCC<T> = Arc<Box<Fn(&mut TransactionParOCC<T>) -> i32 + Send + Sync>>;

#[derive(Clone)]
pub struct Piece {
    callback_: FnPtr,
    pid_:      Pid,
    tname_:    String,
    title_:    &'static str,
    rank_:       usize,
    //R/W sets?
}

#[derive(Clone)]
pub struct PieceOCC<T>
where T: Clone
{
    callback_: FnPtrOCC<T>,
    pid_:      Pid,
    tname_:    String,
    title_:    &'static str,
    rank_:       usize,
    //R/W sets?
}

impl Debug for Piece {
    fn fmt(&self, f: &mut Formatter) -> Result {
        write!(
            f,
            "[pid: {:?}, tname: {:?}, name: {:?}]",
            self.pid_, self.tname_, self.title_
        )
    }
}

impl<T> Debug for PieceOCC<T> 
where T : Clone
{
    fn fmt(&self, f: &mut Formatter) -> Result {
        write!(
            f,
            "[pid: {:?}, tname: {:?}, name: {:?}]",
            self.pid_, self.tname_, self.title_
        )
    }
}

impl Piece {
    pub fn new(
        pid: Pid,
        tname: String,
        cb: FnPtr,
        title: &'static str,
        rank: usize
    ) -> Piece {
        Piece {
            callback_: cb,
            pid_:      pid,
            tname_:    tname,
            title_:    title,
            rank_ :     rank,
        }
    }

    #[cfg_attr(feature = "profile", flame)]
    pub fn run(&mut self, tx : &mut TransactionPar) -> i32 {
        (self.callback_)(tx)
    }

    pub fn id(&self) -> &Pid {
        &self.pid_
    }

    pub fn rank(&self) -> usize {
        self.rank_
    }
}

impl<T> PieceOCC<T>
where T: Clone
{
    pub fn new(
        pid: Pid,
        tname: String,
        cb: FnPtrOCC<T>,
        title: &'static str,
        rank: usize
    ) -> PieceOCC<T> {
        PieceOCC {
            callback_: cb,
            pid_:      pid,
            tname_:    tname,
            title_:    title,
            rank_ :     rank,
        }
    }

    #[cfg_attr(feature = "profile", flame)]
    pub fn run(&mut self, tx : &mut TransactionParOCC<T>) -> i32 {
        (self.callback_)(tx)
    }

    pub fn id(&self) -> &Pid {
        &self.pid_
    }

    pub fn rank(&self) -> usize {
        self.rank_
    }
}


#[derive(Debug, Copy, Clone)]
pub enum PieceState {
    Ready,
    Running,
    Executed,
    Persisted,
    //Checking,
}

#[derive(Clone)]
pub struct DataRecord {
    ptr:    *mut u8,
    layout: Layout,
}

impl DataRecord {
    pub fn new<T: ?Sized>(t: &T) -> Self {
        let ptr = unsafe { mem::transmute::<&T, *const T>(t) };
        DataRecord {
            ptr:    unsafe { ptr as *mut u8 },
            layout: Layout::for_value(t),
        }
    }

    //FIXME: can it be self here?
    pub fn as_log(&self, id: Tid) -> PLog {
        PLog::new(self.ptr, self.layout.clone(), id)
    }
}

unsafe impl Send for DataRecord {}
