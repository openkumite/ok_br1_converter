// PS1-look material patch for three.js: vertex snapping (GTE integer screen
// coords), affine texture mapping, 15-bit colour + ordered dither, nearest filtering.
// Applied through onBeforeCompile so skinning and vertex colours keep working.
(function () {
  const uniforms = { uSnap: { value: new THREE.Vector2(320, 240) }, uPs1: { value: 1 } };

  function patch(material) {
    material.onBeforeCompile = (shader) => {
      shader.uniforms.uSnap = uniforms.uSnap;
      shader.uniforms.uPs1 = uniforms.uPs1;
      shader.vertexShader = shader.vertexShader
        .replace('#include <common>', '#include <common>\nuniform vec2 uSnap;\nuniform float uPs1;\nvarying vec3 vAffine;')
        .replace('#include <project_vertex>', `#include <project_vertex>
          if (uPs1 > 0.5) {
            vec2 ndc = gl_Position.xy / gl_Position.w;
            ndc = floor(ndc * uSnap * 0.5 + 0.5) / (uSnap * 0.5);
            gl_Position.xy = ndc * gl_Position.w;
          }
          #ifdef USE_UV
          // uv*w interpolated perspective-correctly, divided by w per pixel = affine
          vAffine = vec3(vUv * gl_Position.w, gl_Position.w);
          #endif`);
      shader.fragmentShader = shader.fragmentShader
        .replace('#include <common>', '#include <common>\nuniform float uPs1;\nvarying vec3 vAffine;')
        .replace('#include <map_fragment>', `
          #ifdef USE_MAP
          vec2 uvSel = uPs1 > 0.5 ? vAffine.xy / vAffine.z : vUv;
          vec4 sampledDiffuseColor = texture2D(map, uvSel);
          diffuseColor *= sampledDiffuseColor;
          #endif`)
        .replace('#include <dithering_fragment>', `#include <dithering_fragment>
          if (uPs1 > 0.5) {
            const mat4 bayer = mat4(-4.,0.,-3.,1., 2.,-2.,3.,-1., -3.,1.,-4.,0., 3.,-1.,2.,-2.);
            vec2 p = mod(floor(gl_FragCoord.xy), 4.0);
            vec3 c = gl_FragColor.rgb * 255.0 + bayer[int(p.x)][int(p.y)];
            gl_FragColor.rgb = floor(clamp(c, 0.0, 255.0) / 8.0) * 8.0 / 255.0;
          }`);
    };
    material.customProgramCacheKey = () => 'ps1';
    material.needsUpdate = true;
  }

  window.PS1 = {
    uniforms,
    // Prepares a loaded glTF scene: nearest filtering + PS1 patch on every material.
    apply(root) {
      root.traverse((o) => {
        if (!o.isMesh) return;
        for (const m of Array.isArray(o.material) ? o.material : [o.material]) {
          if (m.map) { m.map.magFilter = m.map.minFilter = THREE.NearestFilter; m.map.generateMipmaps = false; m.map.needsUpdate = true; }
          patch(m);
        }
      });
    },
    set enabled(v) { uniforms.uPs1.value = v ? 1 : 0; },
    get enabled() { return uniforms.uPs1.value > 0.5; },
  };
})();
