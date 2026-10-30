# Security

Report vulnerabilities in hblctl itself (for example a parser crash on malicious input, or a safety-gate bypass) through
GitHub's private vulnerability reporting on this repository, not a public issue.

The camera's factory diagnostic service has no authentication: anything that can reach TCP port 30303 on the camera
can use it. That is a property of the camera firmware, not of hblctl; disable the camera's hotspot and USB network
connection when you are not using them.
