# Local Nightly builds coexist with the installed app (com.souffle.desktop.nightly).

.PHONY: help nightly nightly-fresh nightly-dmg

help:
	@echo "make nightly        build Soufflé Nightly (debug) and open it"
	@echo "make nightly-fresh  wipe Nightly data + TCC, then build and open"
	@echo "make nightly-dmg    same as nightly, wrapped in a .dmg"

nightly:
	./scripts/run-nightly.sh

nightly-fresh:
	./scripts/run-nightly.sh --fresh

nightly-dmg:
	./scripts/run-nightly.sh --dmg
