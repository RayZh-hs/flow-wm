package shellembed

import (
	"os"
	"path/filepath"
	"testing"
)

func benchDir(b *testing.B) string {
	b.Helper()
	dir, err := os.MkdirTemp("", "shellembed-bench-")
	if err != nil {
		b.Fatal(err)
	}
	b.Cleanup(func() {
		filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
			if err == nil && d.IsDir() {
				os.Chmod(p, 0o755)
			}
			return nil
		})
		os.RemoveAll(dir)
	})
	return dir
}

func BenchmarkExtractWarm(b *testing.B) {
	if !Available() {
		b.Skip("no embedded UI in this build")
	}
	base := benchDir(b)
	if _, err := Extract(base); err != nil {
		b.Fatal(err)
	}
	b.ResetTimer()
	for b.Loop() {
		if _, err := Extract(base); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkExtractCold(b *testing.B) {
	if !Available() {
		b.Skip("no embedded UI in this build")
	}
	for b.Loop() {
		if _, err := Extract(benchDir(b)); err != nil {
			b.Fatal(err)
		}
	}
}
