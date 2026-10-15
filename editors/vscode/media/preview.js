/* Mem PKB Markdown Preview Script
 * Enhances .mem-pkb-ref elements in VS Code Markdown Preview windows with
 * viewport-aware positioning for .mem-pkb-hover-card tooltips.
 */
(function () {
  function positionHoverCard(refEl) {
    if (!refEl) return;
    var card = refEl.querySelector('.mem-pkb-hover-card');
    if (!card) return;

    // Reset offsets before measuring
    card.style.left = '0px';
    card.style.right = 'auto';
    card.classList.remove('mem-pkb-flip-above');

    var rect = card.getBoundingClientRect();
    var refRect = refEl.getBoundingClientRect();
    var vw = window.innerWidth || document.documentElement.clientWidth || 800;
    var vh = window.innerHeight || document.documentElement.clientHeight || 600;

    // Clamp horizontally so the hover card never overflows the right edge of the preview window
    if (rect.right > vw - 12) {
      var shiftLeft = rect.right - (vw - 12);
      var newLeft = Math.max(-refRect.left + 8, -shiftLeft);
      card.style.left = newLeft + 'px';
    }

    // Flip above the reference link if the hover card would overflow the bottom of the viewport
    if (rect.bottom > vh - 12 && refRect.top > rect.height + 16) {
      card.classList.add('mem-pkb-flip-above');
    }
  }

  function bindPreviewListeners(root) {
    if (!root || !root.querySelectorAll) return;
    var refs = root.querySelectorAll('.mem-pkb-ref');
    for (var i = 0; i < refs.length; i++) {
      var ref = refs[i];
      if (ref.getAttribute('data-pkb-bound') === 'true') continue;
      ref.setAttribute('data-pkb-bound', 'true');

      ref.addEventListener('mouseenter', function (e) {
        var el = e.currentTarget;
        el.classList.add('mem-pkb-hover-active');
        positionHoverCard(el);
      });

      ref.addEventListener('mouseleave', function (e) {
        var el = e.currentTarget;
        el.classList.remove('mem-pkb-hover-active');
      });

      ref.addEventListener('focus', function (e) {
        var el = e.currentTarget;
        el.classList.add('mem-pkb-hover-active');
        positionHoverCard(el);
      });

      ref.addEventListener('blur', function (e) {
        var el = e.currentTarget;
        el.classList.remove('mem-pkb-hover-active');
      });
    }
  }

  if (typeof document !== 'undefined') {
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', function () {
        bindPreviewListeners(document);
      });
    } else {
      bindPreviewListeners(document);
    }

    window.addEventListener('vscode.markdown.updateContent', function () {
      bindPreviewListeners(document);
    });

    if (typeof MutationObserver !== 'undefined' && document.body) {
      var observer = new MutationObserver(function () {
        bindPreviewListeners(document);
      });
      observer.observe(document.body, { childList: true, subtree: true });
    }
  }
})();
