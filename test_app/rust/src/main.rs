use std::sync::Arc;

extern crate clap;
extern crate urcu_ht;
use clap::{App, Arg};

use urcu_ht::RcuHt;

const GLOBAL_KEY_LOOKUP: u32 = 0;

struct ThreadData {
    key_found: u64,
    key_not_found: u64,
}

impl ThreadData {
    fn new() -> Self {
        ThreadData {
            key_found: 0,
            key_not_found: 0,
        }
    }
}

static mut GLOBAL_THREAD_DATA: Vec<ThreadData> = Vec::new();

fn read_rcu(ht: Arc<RcuHt<u32, u32>>, id: u32) {
    let thread = ht.thread();

    let thread_data = unsafe {
        let v = &mut GLOBAL_THREAD_DATA;
        &mut v[id as usize]
    };

    loop {
        let rdlock = thread.rdlock();
        let val = rdlock.get(&GLOBAL_KEY_LOOKUP);
        match val {
            Some(_) => thread_data.key_found += 1,
            None => thread_data.key_not_found += 1,
        }
    }
}

fn main() {
    let mut children = vec![];

    let matches = App::new("My Super Test Program")
        .version("1.0")
        .author("")
        .about("Does awesome things")
        .arg(
            Arg::new("thread")
                .short('t')
                .long("thread")
                .value_name("THREAD")
                .help("Sets a custom thread number")
                .takes_value(true),
        )
        .arg(
            Arg::new("objects")
                .short('c')
                .long("objects")
                .value_name("OBJECTS")
                .help("Sets a custom objects number")
                .takes_value(true),
        )
        .get_matches();

    let threads = matches
        .value_of("thread")
        .unwrap_or("3")
        .parse::<u32>()
        .unwrap();
    let objects = matches
        .value_of("objects")
        .unwrap_or("1")
        .parse::<u32>()
        .unwrap();

    let ht = RcuHt::new(64, 64, 64, false).expect("Cannot allocate RCU hashtable");
    let ht = Arc::new(ht);
    let mut old_thread_data: Vec<ThreadData> = Vec::new();

    for i in 0..threads {
        old_thread_data.push(ThreadData::new());
        unsafe {
            GLOBAL_THREAD_DATA.push(ThreadData::new());
        }

        // Spin up another thread
        let ht = ht.clone();
        children.push(std::thread::spawn(move || {
            read_rcu(ht, i);
        }));
    }

    let thread = ht.thread();
    let mut ht_write = thread.wrlock().unwrap();
    let mut now = std::time::Instant::now();

    loop {
        for i in 0..objects {
            ht_write.insert_or_replace(i, 0);
        }

        std::thread::sleep(std::time::Duration::from_millis(1));

        if now.elapsed().as_secs() >= 1 {
            now = std::time::Instant::now();

            print!("read: ");
            for i in 0..threads {
                let old = &mut old_thread_data[i as usize];
                let thread_data = unsafe {
                    let v = &GLOBAL_THREAD_DATA;
                    &v[i as usize]
                };

                print!(
                    "{} [{} + {}] ",
                    thread_data.key_found + thread_data.key_not_found
                        - old.key_found
                        - old.key_not_found,
                    thread_data.key_not_found - old.key_not_found,
                    thread_data.key_found - old.key_found
                );

                old.key_found = thread_data.key_found;
                old.key_not_found = thread_data.key_not_found;
            }
            println!();
        }

        for i in 0..objects {
            ht_write.remove(&i).expect("Cannot remove key");
        }
    }

    for child in children {
        // Wait for the thread to finish. Returns a result.
        let _ = child.join();
    }
}
