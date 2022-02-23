use std::sync::Arc;

extern crate clap;
extern crate urcu_ht;
use clap::{App, Arg};

use core_affinity::CoreId;
use urcu_ht::RcuHt;

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

fn read_rcu(ht: Arc<RcuHt<u32, u32>>, id: usize) {
    let thread = ht.thread();

    let thread_data = unsafe {
        let v = &mut GLOBAL_THREAD_DATA;
        &mut v[id]
    };

    loop {
        let rdlock = thread.rdlock();
        let val = rdlock.get(&0);
        match val {
            Some(_) => thread_data.key_found += 1,
            None => thread_data.key_not_found += 1,
        }

        #[cfg(feature = "qsbr")]
        thread.quiescent_state();
    }
}

fn main() {
    let mut children = vec![];

    let matches = App::new("My Super Test Program")
        .version("1.0")
        .author("")
        .about("Does awesome things")
        .arg(
            Arg::new("cores")
                .short('c')
                .long("cores")
                .multiple_values(true)
                .value_name("CORES")
                .help("Sets the core list")
                .takes_value(true),
        )
        .arg(
            Arg::new("objects")
                .short('o')
                .long("objects")
                .value_name("OBJECTS")
                .help("Sets a custom objects number")
                .takes_value(true),
        )
        .arg(
            Arg::new("seconds")
                .short('s')
                .long("seconds")
                .value_name("SECONDS")
                .help("Sets a custom run time in seconds")
                .takes_value(true),
        )
        .get_matches();

    // use "cores" option or take all available cores
    let mut cores: Vec<usize> = match matches.values_of("cores") {
        Some(cores) => cores
            .collect::<Vec<&str>>()
            .iter()
            .map(|s| s.parse::<usize>().unwrap())
            .collect::<Vec<usize>>(),
        None => core_affinity::get_core_ids()
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect::<Vec<usize>>(),
    };

    let objects = matches
        .value_of("objects")
        .unwrap_or("1")
        .parse::<u32>()
        .unwrap();
    let seconds = matches
        .value_of("seconds")
        .unwrap_or("10")
        .parse::<u64>()
        .unwrap();

    if cores.len() < 2 {
        println!("There must be at least 2 cores to run this test");
        return;
    }

    if seconds < 1 {
        println!("test should run for at least 1 second");
        return;
    }

    if objects < 1 {
        println!("we must add at least 1 object in database");
        return;
    }

    println!("{} cores used and {objects} objects changed every 1ms.", cores.len());

    let ht = RcuHt::new(64, 64, 64, false).expect("Cannot allocate RCU hashtable");
    let ht = Arc::new(ht);
    let mut old_thread_data: Vec<ThreadData> = Vec::new();

    let mut max_core_id = 0;
    cores.iter().for_each(|c| {
        if c > &max_core_id {
            max_core_id = *c;
        }
    });
    for _i in 0..max_core_id + 1 {
        old_thread_data.push(ThreadData::new());
        unsafe {
            GLOBAL_THREAD_DATA.push(ThreadData::new());
        }
    }

    let master_core_id = cores.pop().unwrap();

    let thread_cores = cores.clone();
    for i in thread_cores {
        core_affinity::set_for_current(CoreId { id: i });
        // Spin up another thread
        let ht = ht.clone();
        children.push(
            std::thread::Builder::new()
                .stack_size(32 * 1024 * 1024)
                .spawn(move || {
                    read_rcu(ht, i);
                })
                .unwrap(),
        );
    }

    core_affinity::set_for_current(CoreId { id: master_core_id });

    let thread = ht.thread();
    let mut ht_write = thread.wrlock().unwrap();
    let mut now = std::time::Instant::now();

    let mut remaining_time = seconds;
    loop {
        for i in 0..objects {
            ht_write.insert_or_replace(i, 0);
        }

        std::thread::sleep(std::time::Duration::from_millis(1));

        if now.elapsed().as_secs() >= 1 {
            now = std::time::Instant::now();

            print!("read: ");
            for i in &cores {
                let old = &mut old_thread_data[*i as usize];
                let thread_data = unsafe {
                    let v = &GLOBAL_THREAD_DATA;
                    &v[*i as usize]
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

            remaining_time -= 1;
            if remaining_time == 0 {
                break;
            }
        }

        for i in 0..objects {
            ht_write.remove(&i).expect("Cannot remove key");
        }
    }

    /* final computation */
    let mut key_found = 0u64;
    let mut key_not_found = 0u64;

    for i in &cores {
        let thread_data = unsafe {
            let v = &GLOBAL_THREAD_DATA;
            &v[*i as usize]
        };

        key_found += thread_data.key_found;
        key_not_found += thread_data.key_not_found;
    }

    println!(
        "total read: {} [{} + {}] ",
        (key_found + key_not_found) / seconds,
        key_not_found / seconds,
        key_found / seconds
    );
}
