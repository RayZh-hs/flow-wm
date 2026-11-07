//go:build withshell

package shellembed

import "embed"

// dist is populated from the repo's quickshell/ tree by `make sync-shell`
// before any tagged build; it is never committed. all: keeps the .dankrev
// revision key, which go:embed would otherwise skip as a dotfile.
//
//go:embed all:dist
var distFS embed.FS
