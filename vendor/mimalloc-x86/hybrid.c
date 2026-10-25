// dmfast's allocator DLL (mimalloc.dll; dmfast.dll imports mi_version and mi_is_redirected from it).
//
// C runtime requests below SPLIT bytes go to mimalloc (linked statically), larger ones to the process
// heap, which the UCRT uses as its own heap and which packs mid-sized blocks without rounding them up to
// a size class (mimalloc's rounding costs ~100 MB on a large world). Speed matches mimalloc alone.
//
// Two sets of hooks, both hot-patched (a long jump written into the CC padding before the function,
// then one atomic 2-byte write of a short jump over its first instruction), so a thread running the
// code meanwhile never sees a torn instruction:
// - ucrtbase.dll's heap entry points (malloc, free, realloc, ... and their *_base forms), so every
//   module that uses the UCRT, including DLLs loaded later and ucrtbase's internal callers, allocates
//   through the split above;
// - ntdll's RtlFreeHeap / RtlSizeHeap / RtlReAllocateHeap / RtlValidateHeap, which every HeapFree /
//   HeapSize / HeapReAlloc / HeapValidate (and LocalFree, ...) ends in. A mimalloc block reaching them
//   (the UCRT's own internals call HeapSize on malloc blocks; another C runtime such as msvcr120, or code
//   using the Win32 heap API, may free or resize malloc memory) is handled by mimalloc.
//
// Blocks are told apart by mimalloc's page map, which only covers mimalloc's own arenas; anything else
// belongs to a Windows heap, including every block allocated before the hooks went in. The UCRT hooks
// call the original ntdll functions directly for heap blocks, skipping that check a second time.
//
// Fail-safe: every hook is checked before any is written; if one function does not start as expected,
// nothing is patched and the process runs on the stock heap. `mimalloc` in DMFAST_DISABLE
// (comma-separated, spaces ignored) does the same.
//
// Build: build.bat (mimalloc v3.5.4, static).
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <errno.h>
#include <stdint.h>
#include <string.h>
#include "mimalloc.h"

#define SPLIT 4096
#define COUNT(a) (sizeof(a) / sizeof *(a))

static HANDLE heap;
static BOOL active;

// mimalloc is initialised before the hooks go in, so its page map can answer for any pointer.
static BOOL ours(const void* p) { return p && mi_is_in_heap_region(p); }

static void* nomem(void* p) {
	if (!p) errno = ENOMEM;
	return p;
}

// ---- ntdll hooks ----------------------------------------------------------------------------------

typedef BOOLEAN(NTAPI* FreeFn)(PVOID, ULONG, PVOID);
typedef SIZE_T(NTAPI* SizeFn)(PVOID, ULONG, LPCVOID);
typedef PVOID(NTAPI* ReAllocFn)(PVOID, ULONG, PVOID, SIZE_T);
typedef BOOLEAN(NTAPI* ValidateFn)(PVOID, ULONG, LPCVOID);
// The original functions (past the hook; set before any hook is written).
static FreeFn rtl_free;
static SizeFn rtl_size;
static ReAllocFn rtl_realloc;
static ValidateFn rtl_validate;

static BOOLEAN NTAPI k_free(PVOID h, ULONG fl, PVOID p) {
	if (ours(p)) { mi_free(p); return TRUE; }
	return rtl_free(h, fl, p);
}
static SIZE_T NTAPI k_size(PVOID h, ULONG fl, LPCVOID p) {
	return ours(p) ? mi_usable_size(p) : rtl_size(h, fl, p);
}
static PVOID NTAPI k_realloc(PVOID h, ULONG fl, PVOID p, SIZE_T n) {
	if (!ours(p)) return rtl_realloc(h, fl, p, n);
	size_t old = mi_usable_size(p);
	void* q = (fl & HEAP_REALLOC_IN_PLACE_ONLY) ? mi_expand(p, n) : mi_realloc(p, n);
	if (q && (fl & HEAP_ZERO_MEMORY) && n > old) memset((char*)q + old, 0, n - old);
	return q;
}
static BOOLEAN NTAPI k_validate(PVOID h, ULONG fl, LPCVOID p) {
	return ours(p) || rtl_validate(h, fl, p);
}

// ---- UCRT hooks ------------------------------------------------------------------------------------

static void* __cdecl h_malloc(size_t n) {
	return nomem(n < SPLIT ? mi_malloc(n) : HeapAlloc(heap, 0, n));
}

static void* __cdecl h_calloc(size_t a, size_t b) {
	if (b && a > SIZE_MAX / b) return nomem(NULL);
	size_t n = a * b;
	return nomem(n < SPLIT ? mi_zalloc(n) : HeapAlloc(heap, HEAP_ZERO_MEMORY, n));
}

static void __cdecl h_free(void* p) {
	if (!p) return;
	if (ours(p)) mi_free(p);
	else rtl_free(heap, 0, p);
}

static size_t __cdecl h_msize(void* p) {
	if (!p) { errno = EINVAL; return (size_t)-1; }
	return ours(p) ? mi_usable_size(p) : rtl_size(heap, 0, p);
}

static void* __cdecl h_realloc(void* p, size_t n) {
	if (!p) return h_malloc(n);
	if (n == 0) { h_free(p); return NULL; }
	if (!ours(p)) // heap blocks stay on the heap (they only grow, or predate the hooks)
		return nomem(rtl_realloc(heap, 0, p, n));
	if (n < SPLIT) return nomem(mi_realloc(p, n));
	// Grown past SPLIT: moves to the heap.
	void* q = HeapAlloc(heap, 0, n);
	if (!q) return nomem(NULL);
	size_t old = mi_usable_size(p);
	memcpy(q, p, old < n ? old : n);
	mi_free(p);
	return q;
}

static void* __cdecl h_recalloc(void* p, size_t a, size_t b) {
	if (b && a > SIZE_MAX / b) return nomem(NULL);
	size_t n = a * b, old = p ? h_msize(p) : 0;
	void* q = h_realloc(p, n);
	if (q && n > old) memset((char*)q + old, 0, n - old);
	return q;
}

static void* __cdecl h_expand(void* p, size_t n) {
	if (!p) { errno = EINVAL; return NULL; }
	return nomem(ours(p) ? mi_expand(p, n) : rtl_realloc(heap, HEAP_REALLOC_IN_PLACE_ONLY, p, n));
}

// ---- hot patching ----------------------------------------------------------------------------------

typedef struct { BYTE* at; void* to; void** original; } Hook;

// Replays a hooked function's first instruction when it is `push imm8`: push imm8 ; jmp f+2.
#pragma pack(push, 1)
typedef struct { BYTE push, imm, jmp; INT32 rel; } Trampoline;
#pragma pack(pop)

// Whether `f` can be hot-patched: five CC/90 padding bytes before it, and a first instruction of exactly
// two bytes that is either `mov edi,edi` (skipped) or `push imm8` (replayed by a trampoline).
static BOOL patchable(const BYTE* f) {
	if (!f) return FALSE;
	for (int i = 1; i <= 5; ++i)
		if (f[-i] != 0xCC && f[-i] != 0x90) return FALSE;
	return (f[0] == 0x8B && f[1] == 0xFF) || f[0] == 0x6A;
}

// Writes one hook into code already made writable.
static void write_hook(const Hook* h) {
	BYTE* pad = h->at - 5;
	pad[0] = 0xE9;
	*(INT32*)(pad + 1) = (INT32)((BYTE*)h->to - (pad + 5));
	MemoryBarrier();
	InterlockedExchange16((SHORT*)h->at, -0x0615); // bytes EB F9: jmp short -7 (to the padding)
	FlushInstructionCache(GetCurrentProcess(), pad, 7);
}

// Puts back the protection of the first `n` hooks' code (in reverse, so a page two hooks share ends up
// as it was before the first change).
static void restore(const Hook* hooks, const DWORD* old, size_t n) {
	DWORD unused;
	while (n--) VirtualProtect(hooks[n].at - 5, 7, old[n], &unused);
}

#define MAX_HOOKS 16

// Checks every hook, sets the originals (building trampolines for `push imm8` starts), makes all their
// code writable, then writes the hooks in order. Changes nothing unless every step before the writes
// succeeds (a process that forbids code changes keeps the stock heap).
static BOOL install(Hook* hooks, size_t n) {
	if (n > MAX_HOOKS) return FALSE;
	size_t pushes = 0;
	for (size_t i = 0; i < n; ++i) {
		if (!patchable(hooks[i].at)) return FALSE;
		pushes += hooks[i].original && hooks[i].at[0] == 0x6A;
	}
	Trampoline* t = NULL;
	if (pushes) {
		t = VirtualAlloc(NULL, pushes * sizeof *t, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
		if (!t) return FALSE;
		Trampoline* next = t;
		for (size_t i = 0; i < n; ++i) {
			if (!hooks[i].original || hooks[i].at[0] != 0x6A) continue;
			*next = (Trampoline){0x6A, hooks[i].at[1], 0xE9, (INT32)((hooks[i].at + 2) - (BYTE*)(next + 1))};
			++next;
		}
		DWORD unused;
		if (!VirtualProtect(t, pushes * sizeof *t, PAGE_EXECUTE_READ, &unused)) {
			VirtualFree(t, 0, MEM_RELEASE);
			return FALSE;
		}
		FlushInstructionCache(GetCurrentProcess(), t, pushes * sizeof *t);
	}
	DWORD old[MAX_HOOKS];
	for (size_t i = 0; i < n; ++i) {
		if (!VirtualProtect(hooks[i].at - 5, 7, PAGE_EXECUTE_READWRITE, &old[i])) {
			restore(hooks, old, i);
			if (t) VirtualFree(t, 0, MEM_RELEASE);
			return FALSE;
		}
	}
	// Originals before any hook can run.
	Trampoline* next = t;
	for (size_t i = 0; i < n; ++i)
		if (hooks[i].original) *hooks[i].original = hooks[i].at[0] == 0x8B ? hooks[i].at + 2 : (BYTE*)next++;
	MemoryBarrier();
	for (size_t i = 0; i < n; ++i) write_hook(&hooks[i]);
	restore(hooks, old, n);
	return TRUE;
}

// `mimalloc` is one of DMFAST_DISABLE's comma-separated keys (spaces ignored).
static BOOL disabled(void) {
	char v[1024];
	DWORD n = GetEnvironmentVariableA("DMFAST_DISABLE", v, sizeof v);
	if (!n || n >= sizeof v) return FALSE;
	char* save = NULL;
	for (char* k = strtok_s(v, ",", &save); k; k = strtok_s(NULL, ",", &save)) {
		char key[16];
		size_t len = 0;
		for (; *k && len < sizeof key - 1; ++k)
			if (*k != ' ') key[len++] = *k;
		key[len] = 0;
		if (!strcmp(key, "mimalloc")) return TRUE;
	}
	return FALSE;
}

BOOL WINAPI DllMain(HINSTANCE self, DWORD reason, LPVOID reserved) {
	(void)reserved;
	if (reason != DLL_PROCESS_ATTACH || disabled()) return TRUE;
	HMODULE ntdll = GetModuleHandleA("ntdll.dll"), ucrt = GetModuleHandleA("ucrtbase.dll");
	if (!ntdll || !ucrt) return TRUE;
	heap = GetProcessHeap();
	// No teardown at process exit. ExitProcess kills the other threads wherever they are (a world's rust_g
	// job threads included) before DLL detach runs; mimalloc's exit routine would then force-collect their
	// heaps and release its thread state while the C runtime's own termination still frees through the
	// hooks below, which crashed DreamDaemon after "Shutdown complete". 2 = skip the exit routine entirely:
	// the arenas stay mapped and every hook keeps working until the process is gone.
	mi_option_set(mi_option_destroy_on_exit, 2);
	// Initialises mimalloc (and its page map) before any hook can ask it about a pointer.
	void* probe = mi_malloc(1);
	if (!probe) return TRUE;
	mi_free(probe);
	// Never unloaded: the hooks point into this DLL.
	HMODULE pinned;
	if (!GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN, (LPCSTR)self, &pinned))
		return TRUE;

#define NT(name, hook, original) {(BYTE*)GetProcAddress(ntdll, name), (void*)(hook), (void**)&(original)}
#define CRT(name, hook) {(BYTE*)GetProcAddress(ucrt, name), (void*)(hook), NULL}
	// ntdll first: the UCRT hooks call the originals and are only safe once heap calls are covered.
	Hook hooks[] = {
		NT("RtlFreeHeap", k_free, rtl_free),
		NT("RtlSizeHeap", k_size, rtl_size),
		NT("RtlReAllocateHeap", k_realloc, rtl_realloc),
		NT("RtlValidateHeap", k_validate, rtl_validate),
		CRT("malloc", h_malloc), CRT("_malloc_base", h_malloc),
		CRT("calloc", h_calloc), CRT("_calloc_base", h_calloc),
		CRT("free", h_free), CRT("_free_base", h_free),
		CRT("realloc", h_realloc), CRT("_realloc_base", h_realloc),
		CRT("_recalloc", h_recalloc), CRT("_msize", h_msize), CRT("_expand", h_expand),
	};
#undef NT
#undef CRT
	active = install(hooks, COUNT(hooks));
	return TRUE;
}

// Whether the split allocator is in (exported as mi_is_redirected, which dmfast.dll's banner reads).
int hybrid_active(void) { return active; }
