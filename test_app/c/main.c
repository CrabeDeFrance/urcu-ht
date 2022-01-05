#include <pthread.h>
#include <stdio.h>
#include <unistd.h>
#include <stdint.h>
#include <string.h>
#include <sys/time.h>
#include "jhash.h"


#define _LGPL_SOURCE
#include <urcu.h>
#include <urcu/rculfhash.h>	/* RCU Lock-free hash table */

#define GLOBAL_HASH_SEED 1234

static int global_done = 0;
static int global_key_lookup = 10;
static int global_thread_count = 3;
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

    /* align to 64 bytes = x86 cache line */
    uint64_t pad24;
    uint64_t pad32;
    uint64_t pad40;
    uint64_t pad48;
    uint64_t pad56;
    uint64_t pad64;
} ;

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

    unsigned long hash = jhash(&global_key_lookup, sizeof(global_key_lookup), GLOBAL_HASH_SEED);

    rcu_register_thread();

    while(!global_done) {
        struct cds_lfht_iter iter;	/* For iteration on hash table */

        urcu_memb_read_lock();

	    cds_lfht_lookup(global_ht, hash, match_cb, &global_key_lookup, &iter);

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
    unsigned long hash = jhash(&global_key_lookup, sizeof(global_key_lookup), GLOBAL_HASH_SEED);
    struct cds_lfht_iter iter;	/* For iteration on hash table */

    urcu_memb_read_lock();

    cds_lfht_lookup(ht, hash, match_cb, &global_key_lookup, &iter);
    struct cds_lfht_node *ht_node = cds_lfht_iter_get_node(&iter);
    if (ht_node) {
        struct mynode *node = caa_container_of(ht_node, struct mynode, node);
        cds_lfht_del(ht, ht_node);
        urcu_memb_call_rcu(&node->rcu_head, free_node_rcu);
    }

    urcu_memb_read_unlock();
}

int main(int argc, char **argv) {
    pthread_attr_t attr;
    int option;

    while((option = getopt(argc, argv, ":t:")) != -1){ //get option from the getopt() method
        switch(option){
            //For option i, r, l, print that these are options
            case 't':
                global_thread_count = atoi(optarg);
                break;
            case '?': //used for some unknown options
                printf("unknown option: %c\n", optopt);
                break;
        }
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

    pthread_t *threads = calloc(global_thread_count, sizeof(pthread_t));
    struct thread_data *thread_data = calloc(global_thread_count, sizeof(struct thread_data));
    struct thread_data *old_thread_data = calloc(global_thread_count, sizeof(struct thread_data));

    for (int i = 0; i < global_thread_count; i++) {
        pthread_create(&threads[i], &attr, read_rcu, &thread_data[i]);
    }

    /* main loop */
    rcu_register_thread();

    __time_t tv_lastsec = 0;

    while (!global_done) {
        struct timeval tv;

        add_node(global_ht, 0, 0);
        add_node(global_ht, 1, 0);
        add_node(global_ht, 2, 0);
        add_node(global_ht, 3, 0);
        add_node(global_ht, 4, 0);
        add_node(global_ht, 5, 0);
        add_node(global_ht, 6, 0);
        add_node(global_ht, 7, 0);
        add_node(global_ht, 8, 0);
        add_node(global_ht, 9, 0);
        add_node(global_ht, global_key_lookup, 0);

        usleep(1);
        gettimeofday(&tv, NULL);

        if (tv_lastsec == 0) {
            tv_lastsec = tv.tv_sec;
        } else {
            if (tv_lastsec != tv.tv_sec) {
                tv_lastsec = tv.tv_sec;

                /* print data */
                printf("read: ");
                for (int i = 0; i < global_thread_count; i++) {
                    printf("%lu [%lu + %lu] ", 
                        thread_data[i].key_found + thread_data[i].key_not_found - old_thread_data[i].key_found - old_thread_data[i].key_not_found,
                        thread_data[i].key_not_found - old_thread_data[i].key_not_found,
                        thread_data[i].key_found - old_thread_data[i].key_found);

                    old_thread_data[i].key_found = thread_data[i].key_found;
                    old_thread_data[i].key_not_found = thread_data[i].key_not_found;
                }
                printf("\n");
            }
        }


        del_node(global_ht, 0);
        del_node(global_ht, 1);
        del_node(global_ht, 2);
        del_node(global_ht, 3);
        del_node(global_ht, 4);
        del_node(global_ht, 5);
        del_node(global_ht, 6);
        del_node(global_ht, 7);
        del_node(global_ht, 8);
        del_node(global_ht, 9);
        del_node(global_ht, global_key_lookup);        
    }

    rcu_unregister_thread();
    
    for (int i = 0; i < global_thread_count; i++) {
        void *ret;
        pthread_join(threads[i], &ret);
    }

    cds_lfht_destroy(global_ht, NULL);

    return 0;
}