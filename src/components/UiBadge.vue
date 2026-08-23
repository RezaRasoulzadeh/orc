<script setup lang="ts">
withDefaults(defineProps<{ tone?: 'neutral' | 'success' | 'warning' | 'danger' | 'info'; dot?: boolean }>(), { tone: 'neutral', dot: false })
</script>

<template><span class="ui-badge" :class="`ui-badge--${tone}`"><i v-if="dot" /> <slot /></span></template>
