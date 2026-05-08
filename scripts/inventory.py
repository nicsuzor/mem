#!/usr/bin/env python3
import os
import re
import sys
from datetime import datetime

def get_emitters():
    emitters = []
    # Pattern to match FactSource::Variant
    pattern = re.compile(r'FactSource::([a-zA-Z0-9]+)')
    
    for root, _, files in os.walk('src'):
        for file in files:
            if not file.endswith('.rs'):
                continue
            path = os.path.join(root, file)
            with open(path, 'r') as f:
                for i, line in enumerate(f, 1):
                    # Skip the definition of the enum
                    if 'enum FactSource' in line:
                        continue
                    matches = pattern.findall(line)
                    for match in matches:
                        emitters.append({
                            'variant': match,
                            'path': path,
                            'line': i,
                            'site': f'{path}:{i}'
                        })
    return emitters

def get_consumer_count(variant, emitter_count=0):
    count = 0
    pattern = re.compile(rf'FactSource::{variant}')
    for root, _, files in os.walk('src'):
        for file in files:
            if not file.endswith('.rs'):
                continue
            path = os.path.join(root, file)
            with open(path, 'r') as f:
                content = f.read()
                count += len(pattern.findall(content))
    return max(0, count - emitter_count)

def main():
    emitters = get_emitters()
    
    inventory_path = 'INVENTORY.md'
    
    # If we are in lint mode, we check if all emitters are in the inventory
    if len(sys.argv) > 1 and sys.argv[1] == '--lint':
        if not os.path.exists(inventory_path):
            print(f"Error: {inventory_path} does not exist.")
            sys.exit(1)
            
        with open(inventory_path, 'r') as f:
            content = f.read()
            
        missing = []
        for e in emitters:
            if e['site'] not in content:
                missing.append(e['site'])
        
        if missing:
            print("The following FactSource emitters are not represented in INVENTORY.md:")
            for m in missing:
                print(f"  {m}")
            sys.exit(1)
        else:
            print("Inventory lint passed.")
            sys.exit(0)

    # Otherwise, generate/update the inventory
    print("# FactSource Inventory")
    print("\nThis file tracks every site in the codebase that emits a `FactSource`.\n")
    print("| FactSource Variant | Producer Site | Consumer Count | Last Modified |")
    print("|---|---|---|---|")
    
    # To keep last modified dates stable, we'd need to read the old inventory.
    # For this task, we'll just use today's date.
    today = datetime.now().strftime('%Y-%m-%d')
    
    for e in emitters:
        emitter_count = sum(1 for em in emitters if em['variant'] == e['variant'])
        count = get_consumer_count(e['variant'], emitter_count)
        print(f"| `{e['variant']}` | `{e['path']}:{e['line']}` | {count} | {today} |")

if __name__ == '__main__':
    main()
