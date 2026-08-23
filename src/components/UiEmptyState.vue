<script setup lang="ts">
defineProps<{ title: string; detail?: string }>()
</script>

<template><div class="ui-empty empty"><span class="ui-empty__mark">//</span><strong>{{ title }}</strong><p v-if="detail">{{ detail }}</p><slot /></div></template>
