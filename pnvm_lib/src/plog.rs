use tcore::TObject;
use txn::Tid;
use std::{
    ptr::Unique,
    mem::size_of,

};
use libc;
use pnvm_sys;


#[repr(C)]
pub struct PLog<T> 
where T: Clone
{
    header: PLogHeader,
    data : PLogData<T>,
}

#[repr(C)]
pub struct PLogHeader {
    log_kind : u16,
    len : usize,
    txn_id: u32
}


#[repr(C)]
pub struct PLogData<T>
where T : Clone
{
    addr : Unique<T>,
    size : usize
}

const LOG_KIND_DATA : u16 = 0;
const LOG_KIND_TXN : u16 = 1;

impl<T> PLog<T> 
where T : Clone
{
    
    pub fn new(obj : &TObject<T>, id : Tid) -> PLog<T> {
        let addr = (obj).get_addr();
        
        PLog {
            header: PLogHeader {
                log_kind :LOG_KIND_DATA,
                len : size_of::<T>(),
                txn_id: id.into()
            },
            data : PLogData {
                addr : addr, 
                size : size_of::<T>()
            }
        }
    }


}

impl Into<libc::iovec> for PLogHeader {
    fn into(self) -> libc::iovec {
        libc::iovec {
            iov_base : &self as *const _ as *mut libc::c_void,
            iov_len : size_of::<PLogHeader>()
        }
    }
}

impl<T> Into<libc::iovec> for PLogData<T>
where T : Clone
{
    fn into(self) -> libc::iovec {
        libc::iovec {
            iov_base : self.addr.as_ptr() as *mut libc::c_void,
            iov_len : self.size 
        }
    }
}

pub fn into_iovec<T :Clone>(log :PLog<T>) -> (libc::iovec, libc::iovec) {
    let mut log = log;
    let iovp_header = libc::iovec {
        iov_base : &mut log.header as *mut _ as *mut libc::c_void,
        iov_len : size_of::<PLogHeader>()
    };

    let iovp_data = libc::iovec {
        iov_base : log.data.addr.as_ptr() as *mut libc::c_void,
        iov_len : log.data.size 
    };

    (iovp_header, iovp_data)
}

pub fn persist_log<T: Clone>(logs :Vec<PLog<T>>){
    let mut iovecs = Vec::with_capacity(logs.len());
    
    
    for (iov_header, iov_data) in logs.into_iter().map(move |log| into_iovec(log)){
        iovecs.push(iov_header);
        iovecs.push(iov_data);
    }

    iovecs.shrink_to_fit();
    debug_assert!(iovecs.capacity() == iovecs.len());
    pnvm_sys::persist_log(&iovecs);
}

pub fn persist_txn(id : u32) {
    let mut iovecs = Vec::with_capacity(2);

    let log = PLog {
        header : PLogHeader{
            log_kind : LOG_KIND_TXN,
            len : size_of::<u32>(),
            txn_id : id
        },

        data : PLogData {
            addr : Unique::from(&id),
            size : size_of::<u32>()
        }
    };

    iovecs.push(log.header.into());
    iovecs.push(log.data.into());

    iovecs.shrink_to_fit();
    debug_assert!(iovecs.capacity() == iovecs.len());
    pnvm_sys::persist_log(&iovecs);
    //pnvm_sys::walk(0, visit_log);
}

extern "C" fn visit_log(buf: *const libc::c_void, len: libc::size_t, _: *mut libc::c_void) -> libc::c_int
{
        println!("------Starting Walk[{:p}, {}]-------", buf, len);
        let mut buf = buf as *mut u8;
        let end = unsafe {buf.add(len)};

        while buf < end {
            let headerp = buf as *const PLogHeader;
            let header = unsafe { &*headerp};

            println!("Entry from tid : {}", header.txn_id);
            println!("Len : {}", header.len);
            println!("Kind : {}", header.log_kind);

            buf = unsafe {buf.add(size_of::<PLogHeader>())};

            let datap = buf as *const u32;
            let data = unsafe { *datap};
            println!("Data : {}", data);

            buf = unsafe {buf.add(size_of::<u32>())};
        }
    0
}