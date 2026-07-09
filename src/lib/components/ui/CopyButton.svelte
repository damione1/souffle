<script lang="ts">
  import { Copy } from "@lucide/svelte";
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

<button onclick={copy} class="btn btn-ghost gap-1.5 px-2.5 py-[5px] text-[12.5px]">
  <Copy size={14} aria-hidden="true" />
  {copied ? $t("ui.copied") : (label ?? $t("ui.copy"))}
</button>
