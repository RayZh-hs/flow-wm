package shm

import "golang.org/x/sys/unix"

func CreateAnonFd(name string) (int, error) {
	return unix.MemfdCreate(name, 0)
}
