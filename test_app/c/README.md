# Compilation and tests

```
mkdir build
cd build
cmake ..
make
./urcu-test-app
```

# Changing default options

## Changing the list of cores to run the test application: -c (default: all available cores)

While 1 core is dedicated to write operations, all other cores are dedicated to read operations.
You can use `grep processor /proc/cpuinfo` to get the list of available core ids.

```
./urcu-test-app -c 0 -c 1
```

## Changing the test duration: --seconds (default: 10)

```
./urcu-test-app --seconds 2
```

## Changing the number of objects added and removed every 1 ms (default: 1)

```
./urcu-test-app --objects 1000
```

