<script setup lang="ts">
import { ref } from 'vue'
const props = withDefaults(defineProps<{ title: string; open?: boolean }>(), { open: false })
const isOpen = ref(props.open)
</script>

<template><details class="ui-disclosure" :open="isOpen" @toggle="isOpen = ($event.target as HTMLDetailsElement).open"><summary>{{ title }}<span aria-hidden="true">{{ isOpen ? '−' : '+' }}</span></summary><div class="ui-disclosure__content"><slot /></div></details></template>
