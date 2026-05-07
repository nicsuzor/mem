export function copyToClipboard(text: string): void {
    if (typeof navigator !== 'undefined' && navigator.clipboard) {
        navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
    } else {
        fallbackCopy(text);
    }
}

function fallbackCopy(text: string) {
    if (typeof document === 'undefined') return;
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    try {
        document.execCommand('copy');
    } catch (err) {
        console.error('Fallback copy failed', err);
    }
    document.body.removeChild(ta);
}

export function formatText(text: string | null | undefined): string {
    if (!text) return "";
    
    // 1. Escape HTML
    let html = text
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;');
        
    // 2. Fix single newlines (unwrap text that was hard-wrapped)
    html = html.replace(/(?<!\n)\n(?!\n)/g, ' ');
    
    // 3. Replace remaining newlines with <br>
    html = html.replace(/\n/g, '<br>');
    
    // 4. Bold
    html = html.replace(/\*\*([^\*]+)\*\*/g, '<strong>$1</strong>');
    
    // 5. Italic
    html = html.replace(/(?<!\*)\*([^\*]+)\*(?!\*)/g, '<em>$1</em>');
    html = html.replace(/\b_([^_]+)_\b/g, '<em>$1</em>');
    
    // 6. Code
    html = html.replace(/`([^`]+)`/g, '<code class="bg-primary/20 text-primary/90 px-1 py-0.5 rounded font-mono text-[0.9em]">$1</code>');
    
    // 7. Links
    html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener noreferrer" class="text-blue-400 hover:underline">$1</a>');

    return html;
}
