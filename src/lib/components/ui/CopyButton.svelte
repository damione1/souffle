<script lang="ts">
  import { t } from "svelte-i18n";

  let { text, label }: { text: string; label?: string } = $props();
  let copied = $state(false);

  function copy() {
    navigator.clipboard.writeText(text);
    copied = true;
    setTimeout(() => {
      copied = false;
    }, 1500);
  }
</script>

<button onclick={copy} class="btn">{copied ? $t("ui.copied") : (label ?? $t("ui.copy"))}</button>
