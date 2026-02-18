import json
import re

with open('codex-rs/core/models.json', 'r', encoding='utf-8') as f:
    data = json.load(f)

# Patterns to remove from all prompts
safety_patterns = [
    # Dirty git worktree block with all sub-bullets
    r'- You may be in a dirty git worktree\.\n\s*\* NEVER revert existing changes you did not make unless explicitly requested.*?\n\s*\* If asked to make a commit or code edits and there are unrelated changes.*?\n\s*\* If the changes are in files you.ve touched recently.*?\n\s*\* If the changes are in unrelated files.*?\n',
    # Do not amend commit
    r'- Do not amend a commit unless explicitly requested to do so\.\n',
    # STOP IMMEDIATELY on unexpected changes
    r'- While you are working, you might notice unexpected changes that you didn.t make\. If this happens, STOP IMMEDIATELY and ask the user how they would like to proceed\.\n',
    # NEVER use destructive commands
    r'- \*\*NEVER\*\* use destructive commands like.*?unless specifically requested or approved by the user\.\n',
    # Git interactive console
    r'- You struggle using the git interactive console\. \*\*ALWAYS\*\* prefer using non-interactive git commands\.\n',
    # 'safe' from 'precise, safe, and helpful'
    r'precise, safe, and helpful',
    # Do not use Python to read/write files (overly restrictive)
    r'- Do not use Python to read/write files when a simple shell command or apply_patch would suffice\.\n',
    # Sandbox and approvals section
    r'## Sandbox and approvals\n.*?(?=\n## |\n# )',
    # NEVER add copyright
    r'- NEVER add copyright or license headers unless specifically requested\.\n',
    # Do not git commit
    r'- Do not `git commit` your changes or create new git branches unless explicitly requested\.\n',
    # NEVER output inline citations
    r'- NEVER output inline citations like.*?open the files in their editor\.\n',
]

def clean_prompt(text):
    if not text:
        return text
    for pattern in safety_patterns:
        text = re.sub(
            pattern,
            lambda m: 'precise and helpful' if 'precise, safe' in m.group() else '',
            text,
            flags=re.DOTALL,
        )
    # Clean up multiple blank lines
    text = re.sub(r'\n{3,}', '\n\n', text)
    return text

count = 0
for model in data['models']:
    slug = model.get('slug', 'unknown')

    if 'base_instructions' in model and model['base_instructions']:
        old = model['base_instructions']
        model['base_instructions'] = clean_prompt(model['base_instructions'])
        if old != model['base_instructions']:
            count += 1
            print('Cleaned base_instructions for: ' + slug)

    mm = model.get('model_messages')
    if mm and isinstance(mm, dict) and mm.get('instructions_template'):
        old = mm['instructions_template']
        mm['instructions_template'] = clean_prompt(mm['instructions_template'])
        if old != mm['instructions_template']:
            count += 1
            print('Cleaned instructions_template for: ' + slug)

print('')
print('Total prompts cleaned: ' + str(count))

with open('codex-rs/core/models.json', 'w', encoding='utf-8') as f:
    json.dump(data, f, indent=2, ensure_ascii=False)

print('File saved successfully')
