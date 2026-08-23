<script setup lang="ts">
withDefaults(defineProps<{ variant?: 'primary' | 'quiet' | 'danger'; size?: 'compact' | 'regular'; loading?: boolean; disabled?: boolean }>(), { variant: 'quiet', size: 'regular', loading: false, disabled: false })
</script>

<template>
  <button class="ui-button" :class="[`ui-button--${variant}`, `ui-button--${size}`]" :disabled="loading || disabled">
    <span v-if="loading" class="ui-spinner" aria-hidden="true" />
    <slot v-if="!loading" />
    <span v-else>WORKING…</span>
  </button>
</template>
