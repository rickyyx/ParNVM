extern crate pnvm_lib;
extern crate rand;

#[macro_use]
extern crate log;
extern crate env_logger;

use std::{
    sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}},
    thread,
};
use rand::{
    thread_rng,
    Rng,
};

use pnvm_lib::{
    txn::*,
    tcore::*,
    tbox::*,
};

const THREAD_NUM : usize = 28;
const OBJ_NUM : usize = 100;
const SET_SIZE: usize = 10;
const ROUND_NUM: usize = 500;

fn main() {
    env_logger::init().unwrap();

    pnvm_lib::tcore::init();
    
    let mtx = Arc::new(Mutex::new(0));
    let mut objs = prepare_data();
    let atomic_cnt = Arc::new(AtomicUsize::new(1));

    let mut handles = vec![];

    for i in 0..THREAD_NUM {
        let read_set = objs.read.pop().unwrap();
        let write_set = objs.write.pop().unwrap();
        let atomic_clone = atomic_cnt.clone();
        let builder = thread::Builder::new()
            .name(format!("TID-{}", i+1));
        
        let mtx = mtx.clone();
        let handle = builder.spawn(move || {
            {
                let _ = mtx.lock().unwrap();
                pnvm_lib::tcore::init();
            }

            for _ in 0..ROUND_NUM {
                let id= atomic_clone.fetch_add(1, Ordering::SeqCst) as u32;
                let tx = &mut Transaction::new(Tid::new(id));

                for read in read_set.iter() {
                    debug!("[THREAD {:} TXN {:}] READ {:}", i+1, id,  tx.read(&read));
                }

                for write in write_set.iter() {
                    tx.write(&write, (i+1) as u32);
                    debug!("[THREAD {:} TXN {:}] WRITE {:}",i+1,  id, i+1);
                }

                info!("[THREAD {:} - TXN {:}] COMMITS {:} ",i+1,  id, tx.try_commit());
            }
        }).unwrap();

        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }
}


pub struct DataSet {
    read : Vec<Vec<TObject<u32>>>,
    write : Vec<Vec<TObject<u32>>>,
}

fn prepare_data() -> DataSet {
    let mut rng = thread_rng();
    let pool : Vec<TObject<u32>> = (0..OBJ_NUM).map(|x| TBox::new(x as u32)).collect();

    let mut dataset = DataSet {
        read : Vec::with_capacity(THREAD_NUM),
        write: Vec::with_capacity(THREAD_NUM),
    };

    for i in 0..THREAD_NUM {
        dataset.read.push(Vec::new());
        dataset.write.push(Vec::new());

        for _ in 0..SET_SIZE {
            let rk : usize = rng.gen_range(0, OBJ_NUM);
            let wk : usize = rng.gen_range(0, OBJ_NUM); 

            dataset.read[i].push(Arc::clone(&pool[rk]));
            dataset.write[i].push(Arc::clone(&pool[wk]));
        }
    }
    dataset
}



