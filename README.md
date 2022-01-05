# urcu-ht

A ReadCopyUpdate (RCU) HashMap implementation. It is build on top of [liburcu].
Userspace RCU is a data synchronization library providing read-side access which scales linearly with the number of cores.
urcu-ht aims to provide a safe wrapper of liburcu.

The default hashing algorithm is currently [wyhash].
There is currently no work done to protected it against HashDos.

Thanks to this implementation, there is no rwlock or mutex in reader threads.
For writer thread, we still need a lock to protect against concurrent insert or remove.

[liburcu]: http://liburcu.org/
[wyhash]: https://docs.rs/wyhash/0.5.0/wyhash/

# How to use

Build documentation (cargo doc) or check out unit tests.