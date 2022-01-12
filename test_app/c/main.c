#define _GNU_SOURCE
#include <sched.h> /* for pthread_setaffinity */
#include <pthread.h>
#include <stdio.h>
#include <unistd.h>
#include <stdint.h>
#include <string.h>
#include <sys/time.h>
#include <getopt.h>
#include "jhash.h"

#define _LGPL_SOURCE
#include <urcu.h>
#include <urcu/rculfhash.h>	/* RCU Lock-free hash table */


#define GLOBAL_HASH_SEED 1234

static struct cds_lfht *global_ht = NULL;

struct mynode {
    struct cds_lfht_node node;	/* Chaining in hash table  */
    struct rcu_head rcu_head;	/* For call_rcu() */
    int key;
    int value;			        /* Node content */
};

struct thread_data {
    uint64_t key_not_found;
    uint64_t key_found;
    uint64_t core_id;

    /* align to 64 bytes = x86 cache line */
    uint64_t pad32;
    uint64_t pad40;
    uint64_t pad48;
    uint64_t pad56;
    uint64_t pad64;
} ;

static int set_affinity(int core_id)
{
    cpu_set_t cpuset;
    CPU_ZERO(&cpuset);
    CPU_SET(core_id, &cpuset);

    pthread_t current_thread = pthread_self();
    return pthread_setaffinity_np(current_thread, sizeof(cpu_set_t), &cpuset);
}

static
int match_cb(struct cds_lfht_node *ht_node, const void *_key)
{
    struct mynode *node =
        caa_container_of(ht_node, struct mynode, node);
    const int *key = _key;

    return *key == node->key;
}

static
void free_node_rcu(struct rcu_head *head)
{
    struct mynode *node = caa_container_of(head, struct mynode, rcu_head);

    free(node);
}


void * read_rcu(void *data)
{
    struct thread_data *thread_data = data;
    int key = 0;

    set_affinity(thread_data->core_id);

    rcu_register_thread();

    while(1) {
        struct cds_lfht_iter iter;	/* For iteration on hash table */

        unsigned long hash = jhash(&key, sizeof(key), GLOBAL_HASH_SEED);

        urcu_memb_read_lock();

        cds_lfht_lookup(global_ht, hash, match_cb, &key, &iter);

        struct cds_lfht_node *ht_node = cds_lfht_iter_get_node(&iter);
        if (ht_node) {
            thread_data->key_found += 1;
        } else {
            thread_data->key_not_found += 1;
        }

        urcu_memb_read_unlock();
    }

    rcu_unregister_thread();

    pthread_exit(NULL);
}

static struct mynode * add_node(struct cds_lfht *ht, int key, int value) {
    unsigned long hash;
    struct mynode *node = malloc(sizeof(*node));
    if (!node) {
        return NULL;
    }

    cds_lfht_node_init(&node->node);

    node->key = key;
    node->value = value;

    hash = jhash(&key, sizeof(key), GLOBAL_HASH_SEED);

    urcu_memb_read_lock();
    cds_lfht_add_replace(ht, hash, match_cb, &key, &node->node);
    urcu_memb_read_unlock();
    return node;
}

static void del_node(struct cds_lfht *ht, int key) {
    unsigned long hash = jhash(&key, sizeof(key), GLOBAL_HASH_SEED);
    struct cds_lfht_iter iter;	/* For iteration on hash table */

    urcu_memb_read_lock();

    cds_lfht_lookup(ht, hash, match_cb, &key, &iter);
    struct cds_lfht_node *ht_node = cds_lfht_iter_get_node(&iter);
    if (ht_node) {
        struct mynode *node = caa_container_of(ht_node, struct mynode, node);
        cds_lfht_del(ht, ht_node);
        urcu_memb_call_rcu(&node->rcu_head, free_node_rcu);
    }

    urcu_memb_read_unlock();
}

extern char *optarg;
extern int optopt;

int main(int argc, char **argv) {
    pthread_attr_t attr;
    int option;
    int objects = 1;
    int seconds = 10;
    int option_index = 0;
    int core_list[512];
    int core_nb = 0;

    static struct option long_options[] = {
        {"core",     required_argument, 0,  'c' },
        {"seconds",  required_argument, 0,  's' },
        {"objects",  required_argument, 0,  'o' },
        {0,         0,                 0,  0 }
    };

    while((option = getopt_long(argc, argv, "c:s:o:", long_options, &option_index)) != -1) { //get option from the getopt() method
        switch(option) {
        //For option i, r, l, print that these are options
        case 'c':
            core_list[core_nb++] = atoi(optarg);
            break;
        case 'o':
            objects = atoi(optarg);
            break;
        case 's':
            seconds = atoi(optarg);
            break;
        case '?': //used for some unknown options
            printf("unknown option: %c\n", optopt);
            break;
        }
    }

    if (core_nb < 2) {
        printf("There must be at least 2 cores\n");
        return 1;
    }

    if (seconds < 5) {
        printf("test should run for at least 5 seconds\n");
        return 1;
    }

    if (objects < 1) {
        printf("we must add at least 1 object in database\n");
        return 1;
    }

    rcu_init();

    /*
     * Allocate hash table.
     */
    global_ht = cds_lfht_new(64, 64, 64, 0, NULL);
    if (!global_ht) {
        printf("Error allocating hash table\n");
        return 1;
    }

    /* create threads */
    if (pthread_attr_init(&attr) != 0) {
        printf("pthread_attr_init");
        return 1;
    }

    int master_core = core_list[--core_nb];

    pthread_t *threads = calloc(core_nb, sizeof(pthread_t));
    struct thread_data *thread_data = calloc(core_nb, sizeof(struct thread_data));
    struct thread_data *old_thread_data = calloc(core_nb, sizeof(struct thread_data));

    /* we wait for a new second to limit complex computation in main loop */
    __time_t tv_lastsec = 0;
    struct timeval tv;

    gettimeofday(&tv, NULL);
    tv_lastsec = tv.tv_sec;

    while (tv.tv_sec == tv_lastsec) {
        usleep(1);
        gettimeofday(&tv, NULL);
    }
    tv_lastsec = tv.tv_sec;

    /* start thread to begin processing */
    for (int i = 0; i < core_nb; i++) {
        thread_data[i].core_id = core_list[i];
        pthread_create(&threads[i], &attr, read_rcu, &thread_data[i]);
    }

    set_affinity(master_core);

    /* main loop */
    rcu_register_thread();

    int remaining_time = seconds;

    while (1) {
        for (int i = 0; i < objects; i++) {
            add_node(global_ht, i, 0);
        }

        usleep(1000);
        gettimeofday(&tv, NULL);

        if (tv_lastsec != tv.tv_sec) {
            tv_lastsec = tv.tv_sec;

            /* print data */
            printf("read: ");
            for (int i = 0; i < core_nb; i++) {
                printf("%lu [%lu + %lu] ",
                       thread_data[i].key_found + thread_data[i].key_not_found - old_thread_data[i].key_found - old_thread_data[i].key_not_found,
                       thread_data[i].key_not_found - old_thread_data[i].key_not_found,
                       thread_data[i].key_found - old_thread_data[i].key_found);

                old_thread_data[i].key_found = thread_data[i].key_found;
                old_thread_data[i].key_not_found = thread_data[i].key_not_found;
            }
            printf("\n");

            remaining_time -= 1;
            if (remaining_time == 0) {
                break;
            }
        }

        for (int i = 0; i < objects; i++) {
            del_node(global_ht, i);
        }
    }

    rcu_unregister_thread();

    /* final computation */
    uint64_t key_found = 0;
    uint64_t key_not_found = 0;

    for (int i = 0; i < core_nb; i++) {
        key_found += thread_data[i].key_found;
        key_not_found += thread_data[i].key_not_found;
    }

    printf(
        "total read: %lu [%lu + %lu]\n",
        (key_found + key_not_found) / seconds,
        key_not_found / seconds,
        key_found / seconds
    );

    return 0;
}