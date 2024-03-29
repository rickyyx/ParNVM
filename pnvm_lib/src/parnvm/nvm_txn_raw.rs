use super::piece::*;

use tcore::{self, *};
use txn::{self, *};

use std::{
    any::Any,
    cell::RefCell,
    collections::{HashMap, HashSet},
    default::Default,
    ptr::NonNull,
    rc::Rc,
    sync::Arc,
    thread,
};

//#[cfg(any(feature= "pmem", feature = "disk"))]
use {
    core::alloc::Layout,
    plog::{self, PLog},
};

//#[cfg(any(feature= "pmem", feature = "disk"))]
extern crate pnvm_sys;

use log;

#[cfg(feature = "profile")]
use flame;

const DEP_DEFAULT_SIZE: usize = 128;

pub struct TransactionParOCCRaw {
    deps_:     HashMap<u32, Arc<TxnInfo>>,
    id_:       Tid,
    status_:   TxState,
    txn_info_: Arc<TxnInfo>,

    //FIXME: store reference instead
    //#[cfg(any(feature= "pmem", feature = "disk"))]
    records_:         Vec<(Box<dyn TRef>, Option<FieldArray>)>,
    tags_:            HashMap<(ObjectId, Operation), TTag>,
    pub early_abort_: bool,
}

impl TransactionParOCCRaw {
    pub fn new(id: Tid) -> TransactionParOCCRaw {
        TransactionParOCCRaw {
            deps_:     HashMap::with_capacity(DEP_DEFAULT_SIZE),
            id_:       id,
            status_:   TxState::EMBRYO,
            txn_info_: Arc::new(TxnInfo::new(id)),

            records_: Vec::new(),

            tags_: HashMap::with_capacity(32),

            early_abort_: false,
        }
    }

    //pub fn new_from_base(txn_base: &TransactionParBaseOCC, tid: Tid, inputs: Box<Any>) -> TransactionParOCC
    //{
    //    let mut txn_base = txn_base.clone();
    //    let pc_cnt = txn_base.all_ps_.len();
    //    let name = txn_base.name_;
    //    let mut pc =Vec::with_capacity(32);
    //    pc.append(&mut txn_base.all_ps_);
    //    TransactionParOCC {
    //        inputs_ : inputs,
    //        outputs_: Vec::with_capacity(32),
    //        all_ps_:    pc,
    //        total_pc_cnt_: pc_cnt as usize,
    //        next_pc_idx_: 0,
    //        name_:      name,
    //        id_:        tid,
    //        status_:    TxState::EMBRYO,
    //        deps_:      HashMap::with_capacity(DEP_DEFAULT_SIZE),
    //        txn_info_:  Arc::new(TxnInfo::new(tid)),
    //
    //        //#[cfg(any(feature= "pmem", feature = "disk"))]
    //        records_ :     Vec::with_capacity(32),

    //        do_piece_drain : false,
    //        tags_: HashMap::with_capacity(16),
    //        early_abort_ : false, // User initiated abort for the whole Txn
    //    }
    //}

    pub fn should_abort(&mut self) {
        warn!("execute_piece::[{:?}] Early Aborting", self.id());
        self.early_abort_ = true;
    }

    /* Implement OCC interface */
    pub fn read<'a, T: 'static + Clone>(&'a mut self, tobj: Box<dyn TRef>) -> &'a T {
        let tag = self.retrieve_tag(tobj.get_id(), tobj.box_clone(), Operation::RWrite);
        tag.add_version(tobj.get_version());
        tag.get_data()
    }

    pub fn write<T: 'static + Clone>(&mut self, tobj: Box<dyn TRef>, val: T) {
        let tag = self.retrieve_tag(tobj.get_id(), tobj.box_clone(), Operation::RWrite);
        tag.write::<T>(val);
    }

    pub fn write_field<T: 'static + Clone>(
        &mut self,
        tobj: Box<dyn TRef>,
        val: T,
        fields: FieldArray,
    ) {
        let tag = self.retrieve_tag(tobj.get_id(), tobj.box_clone(), Operation::RWrite);
        tag.write::<T>(val);
        tag.set_fields(fields);
    }

    #[inline(always)]
    pub fn retrieve_tag(
        &mut self,
        id: &ObjectId,
        tobj_ref: Box<dyn TRef>,
        op: Operation,
    ) -> &mut TTag {
        self.tags_
            .entry((*id, op))
            .or_insert(TTag::new(*id, tobj_ref))
    }

    //FIXME: R->W dependency
    fn add_dep(&mut self) {
        let me: u32 = self.id().into();
        for (_, tag) in self.tags_.iter() {
            let txn_info = tag.tobj_ref_.get_access_info();
            if !txn_info.has_commit() {
                let id: u32 = txn_info.id().into();
                if me != id {
                    /* Do not add myself into it */
                    if !self.deps_.contains_key(&id) && tag.has_write() {
                        warn!("add_dep:: {:?} will wait on {:?}", me, id);
                        self.deps_.insert(id, txn_info);
                    }
                }
            }
        }
    }

    pub fn try_commit_piece(&mut self) -> bool {
        if !self.lock() {
            return self.abort_piece(AbortReason::FailedLocking);
        }

        if !self.check() {
            return self.abort_piece(AbortReason::FailedLocking);
        }

        self.add_dep();

        self.commit_piece()
    }

    fn abort_piece(&mut self, _: AbortReason) -> bool {
        tcore::BenchmarkCounter::abort_piece();
        self.clean_up();
        false
    }

    fn commit_piece(&mut self) -> bool {
        tcore::BenchmarkCounter::success_piece();

        //#[cfg(all(any(feature = "pmem", feature = "disk"), feature = "plog"))]
        self.persist_logs();

        //Install write sets into the underlying data
        self.install_data();

        //Persist the data
        //FIXME: delay the commit until commiting transaction
        #[cfg(any(feature = "pmem", feature = "disk"))]
        {
            #[cfg(feature = "pdrain")]
            self.persist_data();
        }

        //Clean up local data structures.
        self.clean_up();

        true
    }

    #[cfg(any(feature = "pmem", feature = "disk"))]
    fn persist_data(&mut self) {
        for (record, fields) in self.records_.drain(..) {
            #[cfg(feature = "pmem")]
            {
                match fields {
                    Some(ref fields) => {
                        for field in fields.iter() {
                            let paddr = record.get_pmem_field_addr(*field);
                            let size = record.get_field_size(*field);
                            BenchmarkCounter::flush(size);

                            #[cfg(feature = "dir")]
                            pnvm_sys::flush(paddr, size);

                            #[cfg(not(feature = "dir"))]
                            {
                                let vaddr = record.get_field_ptr(*field);
                                pnvm_sys::memcpy_nodrain(paddr, vaddr, size);
                            }
                        }
                    }
                    None => {
                        let paddr = record.get_pmem_addr();
                        let layout = record.get_layout();

                        BenchmarkCounter::flush(layout.size());
                        #[cfg(feature = "dir")]
                        pnvm_sys::flush(paddr, layout.size());

                        #[cfg(not(feature = "dir"))]
                        {
                            let vaddr = record.get_ptr();
                            pnvm_sys::memcpy_nodrain(paddr, vaddr, layout.size());
                        }
                    }
                }
            }

            #[cfg(feature = "disk")]
            {
                let paddr = record.get_pmem_addr();
                let vaddr = record.get_ptr();
                let layout = record.get_layout();
                pnvm_sys::disk_memcpy(paddr, vaddr, layout.size());
                pnvm_sys::disk_msync(paddr, layout.size());
            }
        }

        //for tag in self.tags_.values() {
        //    tag.persist_data(*self.id());
        //}
    }

    fn install_data(&mut self) {
        let id = *self.id();
        let txn_info = self.txn_info().clone();
        for tag in self.tags_.values_mut() {
            tag.commit_data(id);
            //FIXME: R->R also needs to be included
            tag.tobj_ref_.set_access_info(txn_info.clone());
        }
    }

    fn clean_up(&mut self) {
        for (_, tag) in self.tags_.drain() {
            if tag.has_write() && tag.is_lock() {
                tag.tobj_ref_.unlock();
            }
        }
    }

    fn lock(&mut self) -> bool {
        let me = *self.id();
        for tag in self.tags_.values_mut() {
            if !tag.has_write() {
                continue;
            }

            if !tag.lock(me) {
                return false;
            }
            debug!("{:#?} locked!", tag);
        }

        true
    }

    fn check(&mut self) -> bool {
        for tag in self.tags_.values() {
            if !tag.has_read() {
                continue;
            }

            if !tag.check(tag.vers_, self.id().into()) {
                return false;
            }
        }

        true
    }

    #[cfg_attr(feature = "profile", flame)]
    pub fn wait_deps_start(&self, to_run_rank: usize) {
        for (_, dep) in self.deps_.iter() {
            loop {
                /* Busy wait here */
                //#[cfg(feature = "pdrain")]
                //if !dep.has_commit() && !dep.finished(to_run_rank) {
                //    warn!("{:?} waiting  for {:?} start", self.id(), dep.id());
                //    //Why not do log and memcpy here?
                //} else {
                //    break;
                //}
                //#[cfg(not(feature = "pdrain"))]
                if !dep.has_commit() && !dep.has_started(to_run_rank) {
                    warn!("{:?} waiting  for {:?} start", self.id(), dep.id());
                //Why not do log and memcpy here?
                } else {
                    break;
                }
            }
        }
    }

    pub fn cur_rank(&self) -> usize {
        self.txn_info_.rank()
    }

    pub fn activate_txn(&mut self) {
        self.status_ = TxState::ACTIVE;
    }

    //#[cfg(any(feature = "pmem", feature = "disk"))]
    pub fn persist_logs(&mut self) {
        let id = *(self.id());
        let mut logs = vec![];

        for tag in self.tags_.values() {
            if tag.has_write() {
                logs.push(tag.make_log(id));

                #[cfg(not(all(feature = "pmem", feature = "wdrain")))]
                self.records_
                    .push((tag.tobj_ref_.box_clone(), tag.fields_.clone()));
            }
        }
        //        let logs = self.records_.iter().map(|(ptr, layout)| {
        //            match ptr {
        //                Some(ptr) => PLog::new(*ptr, layout.clone(), id),
        //                None => PLog::new_none(layout.clone(), id),
        //            }
        //        }).collect();

        plog::persist_log(logs);
    }

    //#[cfg(any(feature= "pmem", feature = "disk"))]
    //pub fn persist_data(&mut self) {
    //    for (ptr, layout) in self.records_.drain() {
    //        if let Some(ptr) = ptr {
    //            pnvm_sys::flush(ptr, layout.clone());
    //        }
    //    }
    //}

    pub fn update_rank(&self, rank: usize) {
        self.txn_info_.start(rank);
    }

    pub fn id(&self) -> &Tid {
        &self.id_
    }

    pub fn status(&self) -> &TxState {
        &self.status_
    }

    pub fn txn_info(&self) -> &Arc<TxnInfo> {
        &self.txn_info_
    }

    //  pub fn add_wait(&mut self, p: PieceOCC) {
    //      self.wait_ = Some(p)
    //  }

    //#[cfg(any(feature = "pmem", feature = "disk"))]
    #[cfg_attr(feature = "profile", flame)]
    fn persist_log(&self, records: &Vec<DataRecord>) {
        let id = self.id();
        plog::persist_log(records.iter().map(|ref r| r.as_log(*id)).collect());
    }

    //#[cfg(any(feature = "pmem", feature = "disk"))]
    #[cfg_attr(feature = "profile", flame)]
    fn persist_txn(&self) {
        #[cfg(any(feature = "pmem", feature = "disk"))]
        pnvm_sys::drain();

        plog::persist_txn(self.id().into());
        self.txn_info_.persist();
    }

    #[cfg_attr(feature = "profile", flame)]
    pub fn commit(&mut self) {
        self.txn_info_.commit();
        self.status_ = TxState::COMMITTED;
        tcore::BenchmarkCounter::success();

        #[cfg(any(feature = "pmem", feature = "disk"))]
        {
            //Persist data here
            #[cfg(not(any(feature = "wdrain", feature = "pdrain")))]
            {
                self.persist_data();
            }

            self.wait_deps_persist();

            self.persist_txn();
            self.status_ = TxState::PERSIST;
        }
    }

    pub fn abort(&mut self) {
        self.clean_up();
        self.txn_info_.commit();

        #[cfg(any(feature = "pmem", feature = "disk"))]
        self.txn_info_.persist();

        self.status_ = TxState::ABORTED;
        tcore::BenchmarkCounter::abort();
    }

    #[cfg(any(feature = "pmem", feature = "disk"))]
    #[cfg_attr(feature = "profile", flame)]
    fn wait_deps_persist(&self) {
        for (_, dep) in self.deps_.iter() {
            loop {
                /* Busy wait here */
                if !dep.has_persist() {
                    warn!(
                        "wait_deps_persist::{:?} waiting for {:?} to commit",
                        self.id(),
                        dep.id()
                    );
                } else {
                    break;
                }
            }
        }
    }

    #[cfg_attr(feature = "profile", flame)]
    pub fn wait_deps_commit(&self) {
        for (_, dep) in self.deps_.iter() {
            loop {
                /* Busy wait here */
                if !dep.has_commit() {
                    warn!(
                        "wait_deps_commit::{:?} waiting for {:?} to commit",
                        self.id(),
                        dep.id()
                    );
                } else {
                    break;
                }
            }
        }
    }
}
