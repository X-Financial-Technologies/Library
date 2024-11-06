async function displayFiles() {
    try {
        const response = await fetch('/api/files');
        const files = await response.json();
        
        const table = document.getElementById('repo-table');
        
        files.forEach((file, index) => {
            const row = table.insertRow();
            const cell1 = row.insertCell(0);
            const cell2 = row.insertCell(1);
            
            cell1.textContent = `${index + 1}.`;
            cell2.innerHTML = `<a href="${file}">${file}</a>`;
        });
    } catch (error) {
        console.error('Error:', error);
    }
}

displayFiles();