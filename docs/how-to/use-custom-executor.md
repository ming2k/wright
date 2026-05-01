# How to Use a Custom Executor

For parts whose build system is easier to drive with Python or another language:

```toml
[lifecycle.configure]
executor = "python"
script = """
import subprocess, os
subprocess.run(["python", "setup.py", "configure"], check=True)
"""
```

## Executor Definition

Executor definitions live in `executors_dir` (default `/etc/wright/executors`) as TOML files:

```toml
[executor]
name = "python"
description = "Python script executor"
command = "/usr/bin/python3"
args = []
delivery = "tempfile"
tempfile_extension = ".py"
required_paths = ["/usr/lib/python3"]
default_isolation = "strict"
```

See [Configuration](../reference/configuration.md) for the full executor format.
