
use std::{
    collections::HashMap,
    sync::{ RwLock, Arc},
    rc::Rc,
};

use txn::{
    self,
    Transaction,
    Tid,
    TxState,
    AbortReason,
};

use tcore::{self, ObjectId, TObject, TTag};
use plog;

pub struct TransactionOCC<T>
where
    T: Clone,
{
    tid_: Tid,
    state_: TxState,
    deps_: HashMap<ObjectId, TTag<T>>,
    persistent_ : bool
}


impl<T> Transaction<T> for TransactionOCC<T>
where T:Clone
{

     fn try_commit(&mut self) -> bool {
        debug!("Tx[{:?}] is commiting", self.tid_);
        self.state_ = TxState::COMMITTED;

        //Stage 1: lock [TODO: Bounded lock or try_lock syntax]
        if !self.lock() {
            return self.abort(AbortReason::FailedLocking);
        }

        //Stage 2: Check
        if !self.check() {
            return self.abort(AbortReason::FailedLocking);
        }

        //Stage 3: Commit
        self.commit();

        true
    }
    
     fn read(&mut self, tobj: &TObject<T>) -> T {
        let tobj = Arc::clone(tobj);

        let id = tobj.get_id();
        let tag = self.retrieve_tag(id, Arc::clone(&tobj));
        tag.add_version(tobj.get_version());
        if tag.has_write() {
            tag.write_value()
        } else {
            tobj.get_data()
        }
    }

     fn write(&mut self, tobj: &TObject<T>, val: T) {
        let tobj = Arc::clone(tobj);

        let tag = self.retrieve_tag(tobj.get_id(), Arc::clone(&tobj));
        if !tag.has_read() {
            //persist log 
            //let log = PLog(tobj);
             
        }
        tag.write(val);
    }
    /*Non TransactionOCC Functions*/
     fn notrans_read(tobj: &TObject<T>) -> T {
        //let tobj = Arc::clone(tobj);
        tobj.raw_read()
    }

     fn notrans_lock(tobj: &TObject<T>, tid: Tid) -> bool {
        let tobj = Arc::clone(tobj);
        tobj.lock(tid)
    }

}

impl<T> TransactionOCC<T>
where
    T: Clone,
{
    pub fn new(tid_: Tid, use_pmem: bool) -> TransactionOCC<T> {
        txn::mark_start(tid_);
        TransactionOCC {
            tid_,
            state_: TxState::EMBRYO,
            deps_: HashMap::new(),
            persistent_ : use_pmem
        }
    }

    pub fn commit_id(&self) -> Tid {
        self.tid_
    }

    pub fn abort(&mut self, _: AbortReason) -> bool {
        debug!("Tx[{:?}] is aborting.", self.tid_);
        //#[cfg(benchmark)]
        tcore::BenchmarkCounter::abort();
        self.state_ = TxState::ABORTED;
        self.clean_up();
        false
    }


    pub fn lock(&mut self) -> bool {
        let mut locks: Vec<&TTag<T>> = Vec::new();

        for tag in self.deps_.values() {
            if !tag.has_write() {
                continue;
            }
            let _tobj = Arc::clone(&tag.tobj_ref_);
            if !_tobj.lock(self.commit_id()) {
                while let Some(_tag) = locks.pop() {
                    _tag.tobj_ref_.unlock();
                }
                debug!("{:#?} failed to locked!", tag);
                return false;
            } else {
                locks.push(tag);
            }
            debug!("{:#?} locked!", tag);
        }

        true
    }

    fn check(&mut self) -> bool {
        for tag in self.deps_.values() {
            if !tag.has_read() {
                continue;
            }

            if !tag.tobj_ref_.check(&tag.vers_) {
                return false;
            }
        }
        true
    }

    fn commit(&mut self) -> bool {

        //#[cfg(benchmark)]
        tcore::BenchmarkCounter::success();

        //Persist the write set logs 
        if self.persistent_ {
             self.persist_log();
        }
        

        //Install write sets into the underlying data
        self.install_data();
        
        //Persist the data
        if self.persistent_{
            self.persist_data();
        }


        //Persist commit the transaction 
        if self.persistent_{  
            self.persist_commit();
        }
        
        //Clean up local data structures.
        txn::mark_commit(self.commit_id());
        self.clean_up();
        true
    }

    fn persist_commit(&self) {
        //FIXME:: Can it be async? 
        plog::persist_txn(self.commit_id().into());
    }

    fn persist_log(&self) {
        let mut logs = vec![];
        let id = self.commit_id();
        for tag in self.deps_.values() {
            if tag.has_write() {
                logs.push(tag.make_log(id));
            }
        }

        plog::persist_log(logs);

    }

    fn persist_data(&self) {
        for tag in self.deps_.values() {
            tag.persist_data(self.commit_id());  
        }

    }

    fn install_data(&mut self) {
        let id = self.commit_id();
        for tag in self.deps_.values_mut() {
                tag.commit_data(id); 
                //FIXME: delegating to tag for commiting? 
        }

    }


    fn clean_up(&mut self) {
        for tag in self.deps_.values() {
            if tag.has_write() {
                tag.tobj_ref_.unlock();
            }
        }
    }

    pub fn retrieve_tag(&mut self, id: ObjectId, tobj_ref: TObject<T>) -> &mut TTag<T> {
        self.deps_.entry(id).or_insert(TTag::new(id, tobj_ref))
    }

}
