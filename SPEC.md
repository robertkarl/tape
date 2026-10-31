write a transcript saver for pi/claude/codex

motiviation is a note in claude conversations mentioning pruning / deleting old chats

behavior is not confirmed in either codex/claude but it's known that pi doesn't do this.

these transcript are valuable in the future. for either benchmark tasks; training; analysis;

Create a system to save these

tenets

* local first; local only
* encrypted at rest
* encrypted in transit
* simplicity
* 100% compiled languages for this (rust)

design suggestions
- periodic copy from debra <-> a host that simply manages sqlite
- separate; http frontend server to browse it. REALLY simple.
- Use the format in `render_trace.py` in ~/Code/regina/ to get HTML; and view them
- consider just having one proxmox container; lightweight; that pulls transcripts; writes to sqlite; and serves HTML.
- after setup; cut the proxmox containers acces to broader internet.


design / details ON THE TABLE
- ssh-based push/pull moving transcripts
- tailscale as transport (useful for archiving from laptop)


design / details *NOT* ON THE TABLE
- roll out a oauth system
- HTTPS / certs?
- split dns with tailscale
- no password over http shenanigans
- no destructive operations (no 'rm' during planning/implementation/etc) you can move stuff to /tmp

Make no mistakes.

I believe in you.


