package shm

import (
	"unsafe"

	"golang.org/x/sys/unix"
)

// Mirrors FreeBSD libc memfd_create (lib/libc/gen/memfd_create.c):
// shm_open2(SHM_ANON, O_RDWR, 0, SHM_GROW_ON_WRITE, name).
// x/sys/unix has no wrapper for SYS_shm_open2 (571, sys/sys/syscall.h).
const (
	sysShmOpen2    = 571
	shmAnon        = 1   // SHM_ANON ((char *)1), sys/sys/mman.h
	shmGrowOnWrite = 0x2 // SHM_GROW_ON_WRITE, sys/sys/mman.h
)

func CreateAnonFd(name string) (int, error) {
	p, err := unix.BytePtrFromString(name)
	if err != nil {
		return -1, err
	}
	fd, _, errno := unix.Syscall6(sysShmOpen2, shmAnon, unix.O_RDWR, 0, shmGrowOnWrite, uintptr(unsafe.Pointer(p)), 0)
	if errno != 0 {
		return -1, errno
	}
	return int(fd), nil
}
