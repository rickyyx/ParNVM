use super::{entry::*, numeric::*, table::*, tpcc_tables::*, workload_common::*};

use pnvm_lib::lock::lock_txn::*;

use std::{result, str, sync::Arc, time};

use rand::{rngs::SmallRng, thread_rng, Rng};

type Result2PL = result::Result<(), ()>;

fn new_order(
    tx: &mut Transaction2PL,
    tables: &TablesRef,
    w_id: i32,
    d_id: i32,
    c_id: i32,
    ol_cnt: i32,
    src_whs: &[i32],
    item_ids: &[i32],
    qty: &[i32],
    now: i32,
) -> Result2PL {
    let tid = tx.id();
    warn!("[{:?}][TXN-NEWORDER] ------STARTS------", tid);

    /* Acquiring Lock Phase */
    let dis_num = num_district_get();
    let warehouse_ref = tables
        .warehouse
        .retrieve(&w_id, w_id as usize)
        .unwrap()
        .into_table_ref(None, None);
    tx.read_lock_tref(&warehouse_ref)?;
    let customer_ref = tables
        .customer
        .retrieve(&(w_id, d_id, c_id))
        .unwrap()
        .into_table_ref(None, None);
    tx.read_lock_tref(&customer_ref)?;
    let district_ref = tables
        .district
        .retrieve(&(w_id, d_id), (w_id * dis_num + d_id) as usize)
        .unwrap()
        .into_table_ref(None, None);
    tx.write_lock_tref(&district_ref)?;

    //Order
    let order_bucket = tables.order.retrieve_bucket(&(w_id, d_id));
    order_bucket.write_lock(tx.id().into())?;
    tx.add_locks(
        (*order_bucket.get_id(), LockType::Write),
        order_bucket.get_tvers(),
    );

    //NewOrder
    let no_bucket = tables.neworder.retrieve_bucket(&(w_id, d_id));
    no_bucket.write_lock(tx.id().into())?;
    tx.add_locks(
        (*no_bucket.get_id(), LockType::Write),
        no_bucket.get_tvers(),
    );

    let mut item_trefs = vec![];
    let mut stock_trefs = vec![];

    let ol_bucket = tables.orderline.retrieve_bucket(&(w_id, d_id));
    ol_bucket.write_lock(tx.id().into())?;
    tx.add_locks(
        (*ol_bucket.get_id(), LockType::Write),
        ol_bucket.get_tvers(),
    );

    for i in 0..ol_cnt as usize {
        let id = item_ids[i];
        let item_ref = tables
            .item
            .retrieve(&id, id as usize)
            .unwrap()
            .into_table_ref(None, None);
        tx.read_lock_tref(&item_ref)?;
        item_trefs.push(item_ref);

        let stock_ref = tables
            .stock
            .retrieve(&(src_whs[i], item_ids[i]), src_whs[i] as usize)
            .unwrap()
            .into_table_ref(None, None);
        tx.write_lock_tref(&stock_ref)?;
        stock_trefs.push(stock_ref);
    }

    /* Locks Acquired */
    let w_tax = tx.read::<Warehouse>(&warehouse_ref).w_tax;
    let c_discount = tx.read::<Customer>(&customer_ref).c_discount;
    info!("[{:?}][TXN-NEWORDER] Read Customer {:?}", tid, c_id);

    let mut district = tx.read::<District>(&district_ref).clone();
    let o_id = district.d_next_o_id;
    let d_tax = district.d_tax;
    district.d_next_o_id = o_id + 1;
    tx.write_field(&district_ref, district, vec![D_NEXT_O_ID]);

    let mut all_local: i64 = 1;
    for i in 0..ol_cnt as usize {
        if w_id != src_whs[i] {
            all_local = 0;

            #[cfg(feature = "noconflict")]
            panic!("no_conflict!");
        }
    }

    info!(
        "[{:?}][TXN-NEWORDER] push_lock ORDER {:?}, [w_id:{}, d_id:{}, o_id: {}, c_id: {}, cnt {}]",
        tid, o_id, w_id, d_id, o_id, c_id, ol_cnt
    );

    tables.order.push_lock(
        tx,
        Order {
            o_id: o_id,
            o_d_id: d_id,
            o_w_id: w_id,
            o_c_id: c_id,
            o_entry_d: now,
            o_carrier_id: 0,
            o_ol_cnt: Numeric::new(ol_cnt as i64, 1, 0),
            o_all_local: Numeric::new(all_local, 1, 0),
        },
        tables,
    );

    info!("[{:?}][TXN-NEWORDER] push_lock NEWORDER  {:?}", tid, o_id);
    tables.neworder.push_lock(
        tx,
        NewOrder {
            no_o_id: o_id,
            no_d_id: d_id,
            no_w_id: w_id,
        },
        tables,
    );

    for i in 0..ol_cnt as usize {
        //let i_price = tables.item.retrieve(item_ids[i]).unwrap().read(tx).i_price;
        let item_ref = &item_trefs[i];
        let i_price = tx.read::<Item>(&item_ref).i_price;
        let stock_ref = &stock_trefs[i];
        let mut stock = tx.read::<Stock>(&stock_ref).clone();

        let s_quantity = stock.s_quantity;
        let s_remote_cnt = stock.s_remote_cnt;
        let s_order_cnt = stock.s_order_cnt;
        let s_dist = match d_id {
            1 => stock.s_dist_01.clone(),
            2 => stock.s_dist_02.clone(),
            3 => stock.s_dist_03.clone(),
            4 => stock.s_dist_04.clone(),
            5 => stock.s_dist_05.clone(),
            6 => stock.s_dist_06.clone(),
            7 => stock.s_dist_07.clone(),
            8 => stock.s_dist_08.clone(),
            9 => stock.s_dist_09.clone(),
            10 => stock.s_dist_10.clone(),
            _ => panic!("invalid d_id: {}", d_id),
        };

        let qty = Numeric::new(qty[i] as i64, 4, 0);
        stock.s_quantity = if s_quantity > qty {
            stock.s_quantity - qty
        } else {
            stock.s_quantity + Numeric::new(91, 4, 0) - qty
        };

        if src_whs[i] != w_id {
            stock.s_remote_cnt = stock.s_remote_cnt + s_remote_cnt;
        } else {
            stock.s_order_cnt = s_order_cnt + Numeric::new(1, 4, 0);
        }
        info!("[{:?}][TXN-NEWORDER] Update STOCK \n\t {:?}", tid, stock);
        //tx.write(stock_ref, stock);
        tx.write_field(
            &stock_ref,
            stock,
            vec![S_QUANTITY, S_ORDER_CNT, S_REMOTE_CNT],
        );

        let ol_amount = qty
            * i_price
            * (Numeric::new(1, 1, 0) + w_tax + d_tax)
            * (Numeric::new(1, 1, 0) - c_discount);

        //println!("{}", s_dist);
        info!("[{:?}][TXN-NEWORDER] push_lockING ORDERLINE  (w_id:{:?}, d_id:{}, o_id: {}, ol_cnt: {})", tid, w_id, d_id, o_id, i+1);
        tables.orderline.push_lock(
            tx,
            OrderLine {
                ol_o_id: o_id,
                ol_d_id: d_id,
                ol_w_id: w_id,
                ol_number: i as i32 + 1,
                ol_i_id: item_ids[i],
                ol_supply_w_id: src_whs[i],
                ol_delivery_d: 0,
                ol_quantity: qty,
                ol_amount: ol_amount,
                ol_dist_info: s_dist,
            },
            tables,
        );
    }

    Ok(())
}

pub fn new_order_random(
    tx: &mut Transaction2PL,
    tables: &Arc<Tables>,
    w_home: i32,
    rng: &mut SmallRng,
) -> Result2PL {
    let num_wh = num_warehouse_get();
    let num_dis = num_district_get();
    let now = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i32;
    //let w_id = urand(1, NUM_WAREHOUSES, rng);
    let w_id = w_home;
    let d_id = urand(1, num_dis, rng);
    let c_id = nurand(1023, 1, 3000, rng);
    let ol_cnt = urand(5, 15, rng);

    let mut supware = [0 as i32; 15];
    let mut itemid = [0 as i32; 15];
    let mut qty = [0 as i32; 15];

    for i in 0..ol_cnt as usize {
        //supware[i] = if true {
        supware[i] = if urand(1, 100, rng) > 1 {
            w_id
        } else {
            urandexcept(1, num_wh, w_id, rng)
        };

        itemid[i] = nurand(8191, 1, 100000, rng);
        qty[i] = urand(1, 10, rng);

        #[cfg(feature = "noconflict")]
        {
            supware[i] = w_id;
        }
    }

    new_order(
        tx, tables, w_id, d_id, c_id, ol_cnt, &supware, &itemid, &qty, now,
    )
}

//pub fn payment_random(tx: &mut Transaction2PL,
//                      tables: &Arc<Tables>,
//                      w_home : i32,
//                      rng : &mut SmallRng,
//                      )
//    -> bool
//{
//    //let w_id = w_home;
//    let num_wh = num_warehouse_get();
//    let num_dis = num_district_get();
//
//    //let w_id = urand(1, NUM_WAREHOUSES, rng);
//    let w_id = w_home;
//    let d_id = urand(1, num_dis, rng);
//
//    let x = urand(1, 100, rng);
//    let y = urand(1, 100, rng);
//
//    let mut c_w_id : i32;
//    let mut c_d_id : i32;
//
//    if num_wh == 1 || x <= 85 {
//        //85% paying throuhg won house
//        c_w_id = w_id;
//        c_d_id = d_id;
//    } else {
//        //15% paying from remote  warehouse
//        c_w_id =  urandexcept(1, num_wh, w_id, rng);
//        assert!(c_w_id != w_id);
//        c_d_id = urand(1, 10, rng);
//    }
//
//    #[cfg(feature = "noconflict")]
//    {
//        c_w_id = w_id;
//        c_d_id = d_id;
//    }
//
//    let h_amount = rand_numeric(1.00, 5000.00, 10, 2, rng);
//    let h_date = gen_now();
//    //if y<= 60 {
//    if y <= 60 {
//        let c_last = rand_last_name(nurand(255, 0, 999, rng),rng);
//        payment(tx, tables, w_id, d_id, c_w_id , c_d_id, Some(c_last), None, h_amount, h_date, rng)
//    } else {
//        let c_id = nurand(1023, 1, 3000, rng);
//        payment(tx, tables, w_id, d_id, c_w_id , c_d_id, None, Some(c_id), h_amount, h_date, rng)
//    }
//}
//
//fn payment(tx: &mut Transaction2PL,
//           tables : &Arc<Tables>,
//           w_id : i32,
//           d_id : i32,
//           c_w_id : i32,
//           c_d_id : i32,
//           c_last : Option<String>,
//           c_id : Option<i32>,
//           h_amount : Numeric,
//           h_date : i32,
//           rng : &mut SmallRng)
//    -> bool
//{
//    //let wh_num = num_warehouse_get();
//    let dis_num = num_district_get();
//    let tid = tx.id();
//    warn!("[{:?}][TXN-PAYMENT] ------STARTS------ ", tid);
//    /* RW Warehouse */
//    let warehouse_row = tables.warehouse.retrieve(&w_id, w_id as usize).expect("warehouse empty").into_table_ref(None, None);
//    let mut warehouse = match tx.read::<Warehouse>(&warehouse_row) {
//        Ok(val) => val.clone(),
//        Err(_) => {
//            warn!("warehouse read failed lock");
//            return false;
//        }
//    };
//
//    let w_name = warehouse.w_name.clone();
//    warehouse.w_ytd = warehouse.w_ytd +  h_amount;
//    info!("[{:?}][TXN-PAYMENT] Update Warehouse::YTD {:?}", tid, warehouse.w_ytd);
//    if tx.write_field(&warehouse_row, warehouse, vec![W_YTD]).is_err() {
//        warn!("warehouse update failed");
//        return false;
//    }
//    //tx.write(warehouse_row, warehouse);
//
//    /* RW District */
//    let district_row = tables.district.retrieve(&(w_id, d_id),(w_id * dis_num + d_id) as usize ).expect("district empty").into_table_ref(None, None);
//    let mut district = match tx.read::<District>(&district_row) {
//        Ok(v) => v.clone(),
//        Err(_) => {
//            warn!("district reead failed");
//            return false;
//        }
//    };
//    let d_name = district.d_name.clone();
//   // let _d_street_1 = district.d_street_1;
//   // let _d_street_2 = district.d_street_2;
//   // let _d_city = district.d_city;
//   // let _d_state = district.d_state;
//   // let _d_zip = district.d_zip;
//    district.d_ytd = district.d_ytd + h_amount;
//    if tx.write_field(&district_row, district, vec![D_YTD]).is_err() {
//        warn!("district update failed");
//        return false;
//    }
//    //tx.write(district_row,district);
//
//    let c_row = match c_id {
//        Some(c_id) => {
//            /* Case 1 , by C_ID*/
//            info!("[{:?}][TXN-PAYMENT] Getting by id {:?}",
//                   tid, c_id);
//            tables.customer.retrieve(&(c_w_id, c_d_id, c_id)).expect("customer by id empty").into_table_ref(None, None)
//        },
//        None => {
//            assert!(c_last.is_some());
//            info!("[{:?}][TXN-PAYMENT] Getting by Name {:?}",
//                   tid, c_last);
//            let c_last = c_last.unwrap();
//            match tables.customer.find_by_name_id(&(c_last.clone(), c_w_id, c_d_id)) {
//               None=> {
//                   warn!("[{:?}][TXN-PAYMENT] No found Name {:?}", tid, c_last);
//                   return false;
//               }
//               Some(arc) => arc.into_table_ref(None, None)
//            }
//        }
//    };
//
//    let mut c = match tx.read::<Customer>(&c_row) {
//        Ok(v) => v.clone(),
//        Err(_) => {
//            warn!("customer read failed");
//            return false
//        }
//    };
//
//    info!("[{:?}][TXN-PAYMENT] Read Customer\n\t  {:?}", tid, c);
//    let mut c_fields = vec![C_BALANCE, C_YTD_PAYMENT, C_PAYMENT_CNT];
//    c.c_balance -= h_amount;
//    c.c_ytd_payment += h_amount;
//    c.c_payment_cnt += Numeric::new(1, 4, 0);
//    let c_id = c.c_id;
//    let c_credit = c.c_credit.clone();
//    let c_credit = str::from_utf8(&c_credit).unwrap();
//    match c_credit {
//        "BC" => {
//            let new_data_str =  format!("|{},{},{},{},{},{}|",
//                                           c.c_id, c.c_d_id, c.c_w_id, d_id, w_id, h_amount.as_string());
//            let len = new_data_str.len();
//            let new_data = new_data_str.as_bytes();
//            c.c_data.rotate_right(len);
//            c.c_data[0..len].copy_from_slice(new_data);
//
//            c_fields.push(C_DATA);
//        },
//        _ => {},
//    }
//    info!("[{:?}][TXN-PAYMENT] Updating Customer\n\t  {:?}", tid, c);
//    if tx.write_field(&c_row, c, c_fields).is_err() {
//        warn!("customer update failed");
//        return false;
//    }
//    //tx.write(c_row, c);
//
//    /* I History */
//    let h_data = format!("{}    {}", str::from_utf8(&w_name).unwrap(), str::from_utf8(&d_name).unwrap());
//    info!("[{:?}][TXN-PAYMENT] Inserting History::HDATA\t  {:?}", tid, h_data);
//    if tables.history.push_lock(tx,
//                        History::new(
//                             c_id,
//                             c_d_id,
//                             c_w_id,
//                             d_id,
//                             w_id,
//                             h_date,
//                             h_amount,
//                             h_data,
//                        ),
//                        tables).is_err() {
//        warn!("history pushed failed");
//        return false;
//    }
//
//    true
//}
//
//
//pub fn orderstatus_random(tx: &mut Transaction2PL,
//                      tables: &Arc<Tables>,
//                      w_home : i32,
//                      rng : &mut SmallRng,
//                      )
//    -> bool
//{
//    let d_id = urand(1, 10, rng);
//    let w_id = w_home;
//
//    let y = urand(1, 100, rng);
//
//    if y <= 60 {
//        let c_last = rand_last_name(nurand(255, 0, 999, rng),rng);
//        orderstatus(tx, tables, w_id, d_id, w_id , d_id, Some(c_last), None)
//    } else {
//        let c_id = nurand(1023, 1, 3000, rng);
//        orderstatus(tx, tables, w_id, d_id, w_id , d_id, None, Some(c_id))
//    }
//}
//
//
//fn orderstatus(tx: &mut Transaction2PL,
//               tables: &Arc<Tables>,
//               w_id : i32,
//               d_id : i32,
//               c_w_id : i32,
//               c_d_id : i32,
//               c_last : Option<String>,
//               c_id : Option<i32>,
//               )
//    -> bool
//{
//
//    let tid = tx.id();
//    warn!("[{:?}][TXN-ORDERSTATUS] ------STARTS------  ", tid);
//    let c_row = match c_id {
//        Some(c_id) => {
//            tables.customer.retrieve(&(c_w_id, c_d_id, c_id)).expect("customer by id empty").into_table_ref(None, None)
//        },
//
//        None => {
//            assert!(c_last.is_some());
//            info!("[{:?}][ORDER-STATUS] Getting by Name {:?}",
//                   tid, c_last);
//            let c_last = c_last.unwrap();
//            match tables.customer.find_by_name_id(&(c_last.clone(), c_w_id, c_d_id)) {
//               None=> {
//                   warn!("[{:?}][ORDER-STATUS] No found Name {:?}", tid, c_last);
//                   return false;
//               }
//               Some(arc) => arc.into_table_ref(None, None)
//            }
//
//        }
//    };
//
//    let c_id = match tx.read::<Customer>(&c_row) {
//        Ok(c) => c.c_id,
//        Err(_) => return false
//    };
//    //TODO:
//   // let o_row = tables.order
//   //     .retrieve_by_cid(&(c_w_id, c_d_id, c_id))
//   //     .expect(format!("order tempty {:?}", (c_w_id,c_d_id, c_id)).as_str())
//   //     .into_table_ref(None, None);
//    let o_row = match tables.order.retrieve_by_cid(&(c_w_id, c_d_id, c_id)) {
//        None => {
//            warn!("retrieve_by_cid:: corrupted");
//            return false
//        },
//        Some(o_row) => {
//            o_row.into_table_ref(None, None)
//        }
//    };
//
//    let o_id = match tx.read::<Order>(&o_row) {
//        Ok(o) => o.o_id,
//        Err(_) => return false
//    };
//    info!("[{:?}][ORDER-STATUS] GET ORDER FROM CUSTOMER [w_d: {}-{}, o_id: {}, c_id: {}]", tid, c_w_id, c_d_id,o_id, c_id);
//    let ol_arcs = tables.orderline.find_by_oid(&(c_w_id, c_d_id, o_id));
//
//    for ol_arc in ol_arcs {
//        let ol_row = ol_arc.into_table_ref(None, None);
//        let ol = match tx.read::<OrderLine>(&ol_row) {
//            Ok(_ol) => {},
//            Err(_) => return false
//        };
//    }
//
//
//    true
//}
//
//
//
//pub fn delivery(tx: &mut Transaction2PL,
//            tables: &Arc<Tables>,
//            w_id : i32,
//            o_carrier_id: i32,
//            )
//    -> bool
//{
//    let tid = tx.id();
//    info!("[{:?}][DELIVERY STARTs]", tid);
//    warn!("[{:?}][TXN-DELIVERY] ------STARTS------ ", tid);
//    let num_dis = num_district_get();
//
//    for d_id in 1..=num_dis {
//        //TODO:
//        let no_arc = tables.neworder.retrieve_min_oid(&(w_id, d_id));
//        if no_arc.is_some() {
//            let no_row = no_arc.unwrap().into_table_ref(None, None);
//            let no_o_id = match tx.read::<NewOrder>(&no_row) {
//                Ok(no) => no.no_o_id,
//                Err(_) => return false,
//            };
//            //TODO:
//            info!("[{:?}][DELIVERY] DELETING NEWORDER [W_ID: {}, D_ID: {}, O_ID: {}]", tid, w_id, d_id, no_o_id);
//            if !tables.neworder.delete_lock(tx, &(w_id, d_id, no_o_id), tables) {
//                return false;
//            }
//
//            info!("[{:?}][DELIVERY] RETRIEVING ORDER  [W_ID: {}, D_ID: {}, O_ID: {}]", tid, w_id, d_id, no_o_id);
//            let o_row = tables.order.retrieve(&(w_id, d_id, no_o_id)).expect("order empty").into_table_ref(None, None);
//            let mut o = match tx.read::<Order>(&o_row) {
//                Ok(o) => o.clone(),
//                Err(_) => return false,
//            };
//            let o_id = o.o_id;
//            let o_c_id = o.o_c_id;
//
//            o.o_carrier_id = o_carrier_id;
//            //tx.write(o_row, o);
//            if tx.write_field(&o_row, o, vec![O_CARRIER_ID]).is_err() {
//                return false;
//            }
//            let ol_arcs = tables.orderline.find_by_oid(&(w_id, d_id, o_id));
//            let now = gen_now();
//            let mut ol_amount_sum = Numeric::new(0, 6, 2);
//            for ol_arc in ol_arcs {
//                let ol_row = ol_arc.into_table_ref(None, None);
//                let mut ol = match tx.read::<OrderLine>(&ol_row) {
//                    Ok(ol) => ol.clone(),
//                    Err(_) => return false,
//                };
//                ol_amount_sum += ol.ol_amount;
//
//                ol.ol_delivery_d = now;
//                info!("[{:?}][DELIVERY] UPDATEING ORDERLINE [OL_AMOUNT_SUM: {:?}]", tid, ol_amount_sum);
//                //tx.write(ol_row, ol);
//                if tx.write_field(&ol_row, ol, vec![OL_DELIVERY_D]).is_err() {
//                    return false
//                }
//            }
//
//
//            let c_row = tables.customer.retrieve(&(w_id, d_id, o_c_id)).expect("deliver::customer not empty").into_table_ref(None, None);
//            let mut c = match tx.read::<Customer>(&c_row) {
//                Ok(c) => c.clone(),
//                Err(_) => return false,
//            };
//            c.c_balance += ol_amount_sum;
//            c.c_delivery_cnt += Numeric::new(1, 4, 0);
//
//            info!("[{:?}][DELIVERY] UPDATEING CUSTOEMR [CID: {}, DELIVERY_CNT: {:?}]", tid, o_c_id, c.c_delivery_cnt);
//            //tx.write(c_row, c);
//            if tx.write_field(&c_row, c, vec![C_BALANCE, C_DELIVERY_CNT]).is_err() {
//                return false;
//            }
//        }
//    }
//
//    true
//}
//pub fn stocklevel(tx: &mut Transaction2PL,
//              tables : &Arc<Tables>,
//              w_id : i32,
//              d_id : i32,
//              thd: Numeric,
//              )
//    -> bool
//{
//    let tid = tx.id();
//    warn!("[{:?}][TXN-STOCKLVEL] ------STARTS------ ", tid);
//    //let wh_num = num_warehouse_get();
//    let dis_num = num_district_get();
//    let d_row = tables.district.retrieve(&(w_id, d_id), (w_id * dis_num + d_id) as usize).unwrap().into_table_ref(None, None);
//    let d = match tx.read::<District>(&d_row) {
//        Ok(d) => d.clone(),
//        Err(_) => return false,
//    };
//    let d_next_o_id = d.d_next_o_id;
//    info!("[{:?}][STOCK-LEVEL] GETTING NEXT_O_ID [W_D: {}-{}, NEXT_O_ID: {}]", tid, w_id, d_id, d_next_o_id);
//
//    //TODO
//    let ol_arcs = tables.orderline.find_range(w_id, d_id, d_next_o_id - 20, d_next_o_id);
//
//    let mut ol_i_ids = vec![];
//    for ol_arc in ol_arcs {
//        let ol_row = ol_arc.into_table_ref(None, None);
//        let ol = match tx.read::<OrderLine>(&ol_row) {
//            Ok(ol) => ol.clone(),
//            Err(_) => return false,
//        };
//        ol_i_ids.push(ol.ol_i_id);
//        info!("[{:?}][STOCK-LEVEL] RECENT ORDER LINE [W_D: {}-{}, OL_I_ID: {}]", tid, w_id, d_id, ol.ol_i_id);
//    }
//
//    let mut low_stock = 0;
//    for ol_i_id in ol_i_ids.into_iter() {
//        let stock_row = tables.stock.retrieve(&(w_id, ol_i_id), w_id as usize).expect("no stock").into_table_ref(None,None);
//
//        let stock = match tx.read::<Stock>(&stock_row) {
//            Ok(s) => s,
//            Err(_) => return false,
//        };
//        info!("[{:?}][STOCK-LEVEL] STOCK LEVEL CHECK [W_ID:{}, ol_i_id: {}, stock_level: {:?}]", tid, w_id, ol_i_id, stock.s_quantity);
//        if stock.s_quantity < thd {
//            low_stock+=1;
//        }
//    }
//
//    true
//}
