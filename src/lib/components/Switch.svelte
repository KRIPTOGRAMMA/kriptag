<script lang="ts">
  // An on/off switch for a boolean setting.
  //
  // Two reasons it is not a bare <input type="checkbox">, which is what the
  // settings used before:
  //
  // 1. WebKitGTK — the engine the app actually runs in — draws a native checkbox
  //    with the GTK system theme, round and in the system's own colours, ignoring
  //    the app's tokens. The same thing forced a hand-drawn control for the
  //    <select> popups and for the subtask ticks.
  // 2. A setting is a state ("on" / "off"), not a task marked done. The form says
  //    so: position and colour both change, which reads down a long list of
  //    settings in a way a tick does not.
  //
  // The real <input> is kept and only made transparent, so the control is still a
  // checkbox to a screen reader and to the keyboard. display:none would take it
  // out of the accessibility tree entirely.
  //
  // Deliberately NOT used for ticking rows in a list (the rule suggestions in
  // Settings, the subtask checklist): those are selections and completions, not
  // settings, and a switch would misdescribe them.

  type Props = {
    checked: boolean;
    onchange?: (checked: boolean) => void;
    disabled?: boolean;
    ariaLabel?: string;
  };

  let { checked = $bindable(false), onchange, disabled, ariaLabel }: Props = $props();
</script>

<span class="sw" class:disabled>
  <input
    type="checkbox"
    bind:checked
    {disabled}
    aria-label={ariaLabel}
    onchange={(e) => onchange?.(e.currentTarget.checked)}
  />
  <span class="track"></span>
  <span class="knob"></span>
</span>

<style>
  .sw {
    position: relative;
    display: inline-block;
    width: 38px;
    height: 21px;
    /* Never squeezed by the label beside it, which is usually a long sentence. */
    flex: 0 0 auto;
    cursor: pointer;
    vertical-align: -4px;
  }

  .sw.disabled { cursor: default; opacity: 0.5; }

  /* Transparent rather than hidden: it still takes the click, the focus and the
     keyboard, and a screen reader still announces a checkbox with its state. */
  .sw input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    opacity: 0;
    cursor: inherit;
  }

  .track {
    position: absolute;
    inset: 0;
    border-radius: 999px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    transition: background 160ms ease, border-color 160ms ease;
  }

  .knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 15px;
    height: 15px;
    border-radius: 50%;
    /* A raised white knob rather than a flat grey one: it keeps the same colour in
       both states, so the eye tracks the movement instead of a colour change, and
       the shadow is what separates it from the accent fill when switched on. */
    background: var(--bg-card);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.28);
    /* Slightly past its stop and back. The overshoot is what makes the switch feel
       like a thrown lever rather than a value being set; a jump would lose the
       "moved from one side to the other" reading entirely. */
    transition: transform 180ms cubic-bezier(0.34, 1.4, 0.64, 1);
    pointer-events: none;
  }

  .sw:not(.disabled):hover .track { border-color: var(--accent); }

  .sw input:checked ~ .track {
    background: var(--accent);
    border-color: var(--accent);
  }

  /* 38 - 15 - 3*2 = 17: the travel that lands the knob the same 3px from the
     right edge as it starts from on the left. */
  .sw input:checked ~ .knob {
    transform: translateX(17px);
  }

  /* On the wrapper, not the input: the input is transparent and its own ring
     would be drawn around an invisible box. */
  .sw input:focus-visible ~ .track {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  @media (prefers-reduced-motion: reduce) {
    .track, .knob { transition: none; }
  }
</style>
