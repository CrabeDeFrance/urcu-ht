# Compilation and tests

## To test using urcu-memb

### Using default release profile

Default configuration.

```
cargo run --features=memb --release
```

### Using profile-lto

Default configuration using LTO.

```
cargo run --features=memb --profile=release-lto
```

## To test using urcu-qsbr

### Using profile-lto

Configuration providing the better performances.

```
cargo run --features=qsbr --profile=release-lto
```

# Changing default options

## Changing the list of cores to run the test application: --cores (default: all available cores)

While 1 core is dedicated to write operations, all other cores are dedicated to read operations.
You can use `grep processor /proc/cpuinfo` to get the list of available core ids.

```
cargo run --features=qsbr --release -- --cores 0 1
```

## Changing the test duration: --seconds (default: 10)

```
cargo run --features=qsbr --release -- --seconds 2
```

## Changing the number of objects added and removed every 1 ms (default: 1)

```
cargo run --features=qsbr --release -- --objects 1000
```

