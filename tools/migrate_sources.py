#!/usr/bin/env python3
import os
import sys
import re

def migrate_plan(file_path):
    with open(file_path, 'r') as f:
        content = f.read()

    # Simple regex-based migration for [[sources]]
    # This is a bit fragile but should work for most of our standard formats
    
    def replace_source(match):
        source_block = match.group(0)
        
        # Extract fields
        uri_match = re.search(r'uri\s*=\s*"([^"]+)"', source_block)
        sha256_match = re.search(r'sha256\s*=\s*"([^"]+)"', source_block)
        as_match = re.search(r'as\s*=\s*"([^"]+)"', source_block)
        extract_to_match = re.search(r'extract_to\s*=\s*"([^"]+)"', source_block)
        depth_match = re.search(r'depth\s*=\s*(\d+)', source_block)

        if not uri_match:
            return source_block
            
        uri = uri_match.group(1)
        sha256 = sha256_match.group(1) if sha256_match else "SKIP"
        
        new_block = '[[sources]]\n'
        
        if uri.startswith('git+'):
            new_block += 'type = "git"\n'
            # Parse git+url#ref
            git_uri = uri[4:]
            if '#' in git_uri:
                url, ref = git_uri.split('#', 1)
                new_block += f'url = "{url}"\n'
                new_block += f'ref = "{ref}"\n'
            else:
                new_block += f'url = "{git_uri}"\n'
            
            if depth_match:
                new_block += f'depth = {depth_match.group(1)}\n'
        elif uri.startswith('http://') or uri.startswith('https://'):
            new_block += 'type = "http"\n'
            new_block += f'url = "{uri}"\n'
            if sha256 != "SKIP":
                new_block += f'sha256 = "{sha256}"\n'
            if as_match:
                new_block += f'as = "{as_match.group(1)}"\n'
        else:
            # Assume local path
            new_block += 'type = "local"\n'
            new_block += f'path = "{uri}"\n'

        if extract_to_match:
            new_block += f'extract_to = "{extract_to_match.group(1)}"\n'
        
        new_block += "\n"
        return new_block

    new_content = re.sub(r'\[\[sources\]\]\s*(?:(?!\[\[sources\]\]|\[[a-zA-Z]).|\n)*', replace_source, content)
    
    if new_content != content:
        with open(file_path, 'w') as f:
            f.write(new_content)
        print(f"Migrated: {file_path}")
    else:
        print(f"No changes needed: {file_path}")

def main():
    if len(sys.argv) > 1:
        for path in sys.argv[1:]:
            if os.path.isfile(path):
                migrate_plan(path)
            elif os.path.isdir(path):
                for root, _, files in os.walk(path):
                    for f in files:
                        if f == "plan.toml":
                            migrate_plan(os.path.join(root, f))
    else:
        print("Usage: migrate_sources.py <file_or_directory> [...]")

if __name__ == "__main__":
    main()
