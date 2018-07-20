
use txn::{Tid};


#[derive(Eq, PartialEq,  Hash, Debug, Clone)]
pub struct Pid(u32);

impl Pid {
    pub fn new(pid: u32) -> Pid {
        Pid(pid)
    }
}


pub struct Piece
{
    callback_ : Box<FnMut()->i32>,
    pid_ : Pid,
    tid_ : Tid,
    //R/W sets?
}


impl Piece
{
    pub fn new(pid : Pid, tid: Tid, cb : Box<FnMut()->i32>) -> Piece {
        Piece {
            callback_ : cb,
            pid_: pid,
            tid_ : tid
        }
    }
    

    pub fn run(&mut self) -> i32 {
        (self.callback_)()
    }

    pub fn id(&self) -> &Pid {
        &self.pid_
    }
}

#[derive(Copy, Clone)]
pub enum PieceState {
    Ready,
    Running,
    Executed,
    Persisted,
}

