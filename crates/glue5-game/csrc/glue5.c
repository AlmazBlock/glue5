#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "game.h"
#include "glue5.h"
#include "wasm-rt-exceptions.h"
#include "wasm-rt-impl.h"

void glue5_init(void) {
  wasm_rt_init();
}

static glue5_wait32_fn glue5_wait32;
static glue5_wait64_fn glue5_wait64;
static glue5_notify_fn glue5_notify;

void glue5_set_atomics(glue5_wait32_fn wait32, glue5_wait64_fn wait64, glue5_notify_fn notify) {
  glue5_wait32 = wait32;
  glue5_wait64 = wait64;
  glue5_notify = notify;
}

uint32_t wasm_rt_atomic_wait32(void* memory, uint64_t addr, uint32_t expected, int64_t timeout_ns) {
  return glue5_wait32(memory, addr, expected, timeout_ns);
}

uint32_t wasm_rt_atomic_wait64(void* memory, uint64_t addr, uint64_t expected, int64_t timeout_ns) {
  return glue5_wait64(memory, addr, expected, timeout_ns);
}

uint32_t wasm_rt_atomic_notify(void* memory, uint64_t addr, uint32_t count) {
  return glue5_notify(memory, addr, count);
}

size_t glue5_memory_struct_size(void) {
  return sizeof(wasm_rt_shared_memory_t);
}

/* The host owns the allocation (reserved up front, so the base never moves
 * while other threads run); this just describes it to the generated code. */
void glue5_memory_init(wasm_rt_shared_memory_t* memory,
                       uint8_t* data,
                       uint64_t pages,
                       uint64_t max_pages) {
  memset(memory, 0, sizeof *memory);
  memory->data = (_Atomic volatile uint8_t*)data;
  memory->data_end = (_Atomic volatile uint8_t*)(data + max_pages * WASM_DEFAULT_PAGE_SIZE);
  memory->page_size = WASM_DEFAULT_PAGE_SIZE;
  memory->pages = pages;
  memory->max_pages = max_pages;
  memory->size = pages * WASM_DEFAULT_PAGE_SIZE;
  memory->is64 = true;
#if defined(_WIN32)
  InitializeCriticalSection(&memory->mem_lock);
#else
  pthread_mutex_init(&memory->mem_lock, NULL);
#endif
}

wasm_rt_shared_memory_t* w2c_env_memory(struct w2c_env* env) {
  return env->memory;
}

/* Creates this thread's instance; runs the start function (which initialises
 * memory only on the first instance). Returns the trap code. */
uint32_t glue5_instance_new(struct w2c_env* env) {
  w2c_game* instance = calloc(1, sizeof *instance);
  if (!instance) {
    abort();
  }
  env->instance = instance;
  GLUE5_TRY(wasm2c_game_instantiate(
      instance, env, (struct w2c_wasi__snapshot__preview1*)env));
}

void glue5_instance_free(struct w2c_env* env) {
  if (env->instance) {
    wasm2c_game_free(env->instance);
    free(env->instance);
    env->instance = NULL;
  }
}

/* Calls table function `index` as `void* f(void*)` (a pthread start routine). */
uint32_t glue5_call_ptr_ptr(struct w2c_env* env, uint64_t index, uint64_t arg, uint64_t* out) {
  wasm_rt_funcref_table_t* table = w2c_game_0x5F_indirect_function_table(env->instance);
  if (index >= table->size || !table->data[index].func) {
    fprintf(stderr, "glue5: bad function index %llu\n", (unsigned long long)index);
    abort();
  }
  const wasm_rt_funcref_t* f = &table->data[index];
  GLUE5_TRY(*out = ((u64(*)(void*, u64))f->func)(f->module_instance, arg));
}

/* Emscripten's longjmp: back to the innermost invoke_* frame. */
_Noreturn void glue5_longjmp(struct w2c_env* env) {
  if (!env->longjmp_target) {
    fprintf(stderr, "glue5: longjmp without a setjmp frame\n");
    abort();
  }
  GLUE5_LONGJMP(*(jmp_buf*)env->longjmp_target, 1);
}

_Noreturn void glue5_unwind(void) {
  WASM_RT_LONGJMP(g_wasm_rt_jmp_buf, GLUE5_TRAP_UNWIND);
  abort();
}
