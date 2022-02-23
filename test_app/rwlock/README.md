# Compilation and tests

## Using default release profile

```
cargo run --release
```

## Using profile-lto

```
cargo run --profile=release-lto
```

# Changing default options

## Changing the list of cores to run the test application: --cores (default: all available cores)

While 1 core is dedicated to write operations, all other cores are dedicated to read operations.
You can use `grep processor /proc/cpuinfo` to get the list of available core ids.

```
cargo run --release -- --cores 0 1
```

## Changing the test duration: --seconds (default: 10)

```
cargo run --release -- --seconds 2
```

## Changing the number of objects added and removed every 1 ms (default: 1)

```
cargo run --release -- --objects 1000
```

