# FLDR Release Checklist

FLDR is licensed under AGPL-3.0-or-later.
Original Atlas copyrights and third-party licenses remain applicable.

## Distribution requirements

- Distribute the verified Windows x64 executable.
- Include LICENSE, NOTICE and applicable third-party notices.
- Provide complete matching corresponding source and build instructions.
- Preserve upstream copyrights and modification notices.
- Review redistribution rights for separately licensed artwork.
- Generate SHA-256 checksums for release archives.
- Publish through the official FLDR GitHub repository.

## License verification

Regenerate the dependency inventory when dependencies or assets change:

    python scripts/generate-third-party.py

Verify recorded licensing evidence:

    python scripts/check-legal.py

Run license collection regression tests:

    python scripts/test-third-party-notices.py

Automated verification does not establish redistribution permission
or complete compliance with corresponding-source obligations.

## Official locations

Source: https://github.com/digart11/FLDR
Releases: https://github.com/digart11/FLDR/releases

## Distribution scope

FLDR uses portable Windows distribution.
No Microsoft Store package, commercial edition or installer is provided.
Optional external components retain their separate license terms.
