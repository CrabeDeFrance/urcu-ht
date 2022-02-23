# Test applications

In this directory, you will find 3 test applications, in Rust and C languages :
* C using liburcu (reference in C lang) : testapp/c
* Rust rwlock (reference using rust standard crate) : testapp/rwlock
* Rust using urcu-ht (this library) : testapp/rust

C version works only with urcu-memb. Rust version can work both with urcu-memb or qsbr.
Qsbr version is a bit faster, but there is no figure using this flavor below.

# Interpreting results

Each second, the application will display a new line with many numbers.

```
read: 33253817 [21420 + 33232397] 24438031 [16692 + 24421339] 24376409 [17006 + 24359406] 
```

This is a per core list. Each core prints the number of read operations done during one second :
```
read 33253817 [21420 + 33232397]
```
This means "33253817" read operations were done. Values [21420 + 33232397] are [miss + hit] details (since the test application periodically removes and adds new items in the main hashtable).

The application runs for 10s by default, then prints a last line "total read", computing the sum of results of all cores, then printing the average sum per second.

# Typical performance results (on my old i5 4 cores @3.3 GHz)

## C

Results are very very good, and scales perfectly, tested with clang 10 and gcc 9

```
read: 61545335 [46311 + 61499024] 61310876 [50096 + 61260780] 61439291 [48109 + 61391182] 
total read: 186905145 [132242 + 186772902]
```

So each read loop takes approximately 53 CPU cycles, performing an average of 186 millions of loops per second.

## Rust rwlock

As expected, performance are very poor compared to urcu.

```
read: 3202050 [221068 + 2980982] 3227698 [275246 + 2952452] 3119474 [261728 + 2857746] 
total read: 9717451 [929267 + 8788183] 
```

So ~10 millions read compared to 186 millions (19x slower).
Each read loop takes approximately 1030 CPU cycles.

Of course, performance decrease quickly with the number of cores. With only 2 cores, we can reach 25 millions. But starting at 3 cores, we reach a total limit of 10 millions operations (meaning adding more cores decrease the global performances !).

## Rust urcu

Performance are far better than rwlock but unfortunatly much slower than C.

```
read: 28346999 [17317 + 28329682] 23122632 [14596 + 23108037] 24190793 [15324 + 24175471] 
total read: 74074488 [57461 + 74017026]
```

So a total of 74 millions read (2.5x slower than C, but still 10x better than the simple rwlock).
In this case, average CPU cycles per loop at 4 cores is 117. 

That means the application does not scale very well too. When we run this test with only 2 cores, we got something closer to C version : 50m read, so 64 cycles per read (80% of C version). But increasing number of cores reduces the performances of each core. 
When profiling, it looks like there is some kind of data or instruction cache pollution. Rust assembly code seems to be a bit larger than C and does more function calls. This could have this huge impact when trying to reach this high level of performance.

## Changing the number of objects added & removed every ms

It is possible to ask for adding more or less hashmap objects per millisecond. By default, only 1 object is removed then added and each read thread does a lookup on it. When changing more objects, this impacts performances of both C and Rust test applications. With 1000 objects added and removed every 1ms, performance of both test applications are very close, with around 65 millions read/s for C and 60 millions for Rust.

## Conclusions

We saw urcu is far better than rwlock. In a specific test case, we hashmap is almost unchanged during the test, C outperform rust by factor 2.5. But in a context with more changes in the hashmap, performance of C and Rust versions are very close.
Since this test app is not very realisitc, we could expect performance issues to come from an other part of the software in a real application.
But any comment to improve performances of this rust version is welcome :)
