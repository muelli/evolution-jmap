/* SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * This program checks that ThreadSanitizer sees the locks of the GLib in
 * the image. It runs one mode per call, given as the first argument:
 *   locked     Two threads increment a counter under a GMutex. Expect no report.
 *   queue      One thread passes a buffer to another through a GAsyncQueue.
 *              Expect no report.
 *   unlocked   Two threads increment a counter with no lock. Expect a report.
 * With an uninstrumented GLib, "locked" and "queue" also give a report.
 */

#include <glib.h>
#include <string.h>

#define ROUNDS 100000

static GMutex lock;
static long counter;

static gpointer
increment_locked (gpointer data)
{
	for (int i = 0; i < ROUNDS; i++) {
		g_mutex_lock (&lock);
		counter++;
		g_mutex_unlock (&lock);
	}
	return NULL;
}

static gpointer
increment_unlocked (gpointer data)
{
	for (int i = 0; i < ROUNDS; i++)
		counter++;
	return NULL;
}

static gpointer
fill_and_push (gpointer data)
{
	char *buffer = g_malloc (64);

	strcpy (buffer, "written by the producer");
	g_async_queue_push (data, buffer);
	return NULL;
}

static int
run_pair (GThreadFunc func)
{
	GThread *a = g_thread_new ("a", func, NULL);
	GThread *b = g_thread_new ("b", func, NULL);

	g_thread_join (a);
	g_thread_join (b);
	return counter == 2 * ROUNDS ? 0 : 1;
}

static int
run_queue (void)
{
	GAsyncQueue *queue = g_async_queue_new ();
	GThread *producer = g_thread_new ("producer", fill_and_push, queue);
	char *buffer = g_async_queue_pop (queue);
	int status = strcmp (buffer, "written by the producer") == 0 ? 0 : 1;

	g_thread_join (producer);
	g_free (buffer);
	g_async_queue_unref (queue);
	return status;
}

int
main (int argc, char **argv)
{
	if (argc == 2 && strcmp (argv[1], "locked") == 0)
		return run_pair (increment_locked);
	if (argc == 2 && strcmp (argv[1], "queue") == 0)
		return run_queue ();
	if (argc == 2 && strcmp (argv[1], "unlocked") == 0) {
		run_pair (increment_unlocked);
		return 0;
	}
	g_printerr ("usage: %s locked|queue|unlocked\n", argv[0]);
	return 2;
}
