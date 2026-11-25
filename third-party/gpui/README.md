# Marsa GPUI dependency sources

This branch pins the released source inputs and maintained compatibility
patches used by Marsa's native macOS GUI. UPSTREAM.tsv records registry archive
SHA-256 digests. Each crate retains its upstream licence and notices.

The first commit contains the original archives. Later commits record the
changes for native GPUI and AccessKit, shared dependencies, localisation and
LSP, and the component graph. Marsa's Cargo.lock and dependency policy remain
in the application repository. Rebuild and audit from that workspace.

Use the native macOS source graph for Marsa. Linux, Windows, and browser
integrations have separate optional features for future platform work.

Marsa places vertical scrollbar tracks and editor overview rulers at the inline
end in both interface directions. The scrollbar tests cover mirrored hit
geometry, thumb dragging, and the corner where two tracks meet.
